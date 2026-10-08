use std::path::Path;

use arc_core::{BearerToken, Config};
use arc_mcp::HttpBinding;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::oneshot;

struct Server {
    base: String,
    url: String,
    client: reqwest::Client,
    _stop: oneshot::Sender<()>,
    _root: tempfile::TempDir,
}

fn config_in(root: &Path, mcp_token: Option<BearerToken>) -> Config {
    Config {
        notes_dir: root.join("notes"),
        cache_dir: root.join("cache"),
        api_token: String::new(),
        editor: String::new(),
        repositories_directory: root.to_path_buf(),
        mux: Default::default(),
        notifier: Default::default(),
        mcp_token,
    }
}

async fn start(token: Option<&str>) -> Server {
    let root = tempfile::tempdir().unwrap();
    let config = config_in(root.path(), token.map(|t| BearerToken::parse(t).unwrap()));
    std::fs::create_dir_all(&config.notes_dir).unwrap();
    arc_index::open(&config.cache_dir).unwrap();
    let binding = HttpBinding::new(
        "127.0.0.1:0".parse().unwrap(),
        vec![],
        config.mcp_token.clone(),
    )
    .unwrap();
    let listener = binding.listen().await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(arc_mcp::http::serve(listener, config, async move {
        let _ = stopped.await;
    }));
    Server {
        base: format!("http://{addr}"),
        url: format!("http://{addr}{}", arc_mcp::MCP_PATH),
        client: reqwest::Client::new(),
        _stop: stop,
        _root: root,
    }
}

impl Server {
    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body)
    }

    async fn rpc(&self, id: u64, method: &str, params: Value, token: Option<&str>) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let request = match token {
            Some(token) => self.post(&body).bearer_auth(token),
            None => self.post(&body),
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{method}");
        let reply: Value = response.json().await.unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    async fn initialize(&self, token: Option<&str>) -> Value {
        let params = json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"},
        });
        self.rpc(1, "initialize", params, token).await
    }

    async fn status_with(&self, authorization: Option<&str>) -> StatusCode {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
        let request = match authorization {
            Some(value) => self.post(&body).header("Authorization", value),
            None => self.post(&body),
        };
        request.send().await.unwrap().status()
    }
}

fn tool_text(reply: &Value) -> &str {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    reply["result"]["content"][0]["text"].as_str().unwrap()
}

#[tokio::test]
async fn serves_the_tools_over_streamable_http() {
    let server = start(None).await;
    let init = server.initialize(None).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "arc", "{init}");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("never `cwd`"),
        "{init}"
    );

    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let status = server.post(&initialized).send().await.unwrap().status();
    assert_eq!(status, StatusCode::ACCEPTED);

    let tools = server.rpc(2, "tools/list", json!({}), None).await;
    let tools = tools["result"]["tools"].as_array().unwrap();
    let current = tools.iter().find(|t| t["name"] == "current_story").unwrap();
    assert!(
        current["inputSchema"]["properties"]["branch"].is_object(),
        "{current}"
    );
    assert!(tools.iter().any(|t| t["name"] == "add_todo"));

    let added = server
        .rpc(
            3,
            "tools/call",
            json!({"name": "add_todo", "arguments": {"text": "over http", "global": true}}),
            None,
        )
        .await;
    assert!(tool_text(&added).starts_with("added "), "{added}");

    let rows = server
        .rpc(
            4,
            "tools/call",
            json!({"name": "query", "arguments": {"sql": "SELECT text FROM todos"}}),
            None,
        )
        .await;
    assert_eq!(tool_text(&rows), r#"[{"text":"over http"}]"#);

    let story = server
        .rpc(
            5,
            "tools/call",
            json!({"name": "current_story", "arguments": {}}),
            None,
        )
        .await;
    assert_eq!(story["result"]["isError"], true, "{story}");
}

#[tokio::test]
async fn rejects_missing_or_wrong_bearer_tokens() {
    let server = start(Some("s3cret")).await;
    assert_eq!(server.status_with(None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.status_with(Some("Bearer nope")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.status_with(Some("Basic s3cret")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.status_with(Some("Bearer s3cret")).await,
        StatusCode::OK
    );

    let init = server.initialize(Some("s3cret")).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "arc", "{init}");
}

#[tokio::test]
async fn rejects_unexpected_host_headers() {
    let server = start(None).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
    let status = server
        .post(&body)
        .header("Host", "evil.example")
        .send()
        .await
        .unwrap()
        .status();
    assert!(status.is_client_error(), "{status}");
}

#[tokio::test]
async fn every_method_and_path_needs_the_token() {
    let server = start(Some("s3cret")).await;
    let nested = format!("{}/x", server.url);
    let unknown = format!("{}/elsewhere", server.base);
    for (method, url) in [
        (reqwest::Method::GET, &server.url),
        (reqwest::Method::DELETE, &server.url),
        (reqwest::Method::OPTIONS, &server.url),
        (reqwest::Method::POST, &nested),
        (reqwest::Method::GET, &unknown),
    ] {
        let status = server
            .client
            .request(method.clone(), url)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {url}");
    }
}

#[tokio::test]
async fn oversized_bodies_get_413_after_auth() {
    let server = start(Some("s3cret")).await;
    let padding = "x".repeat(2 * 1024 * 1024);
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"padding": padding}});
    let unauthenticated = server.post(&body).send().await.unwrap().status();
    assert_eq!(unauthenticated, StatusCode::UNAUTHORIZED);
    let status = server
        .post(&body)
        .bearer_auth("s3cret")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn remote_session_tools_refuse_cwd() {
    let server = start(None).await;
    let cwd = std::env::temp_dir().display().to_string();
    let story = server
        .rpc(
            1,
            "tools/call",
            json!({"name": "current_story", "arguments": {"cwd": cwd}}),
            None,
        )
        .await;
    assert_eq!(story["result"]["isError"], true, "{story}");
    let text = story["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("can't see your filesystem"), "{text}");
}
