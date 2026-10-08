use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_core::{BearerToken, Config};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

use crate::ArcMcp;

pub const MCP_PATH: &str = "/mcp";
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http(HttpBinding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpBinding {
    pub addr: SocketAddr,
    pub extra_hosts: Vec<String>,
}

pub async fn run(config: &Config, binding: HttpBinding) -> Result<()> {
    arc_index::open(&config.cache_dir)?;
    let listener = TcpListener::bind(binding.addr)
        .await
        .with_context(|| format!("couldn't bind {}", binding.addr))?;
    let mut terminate = signal(SignalKind::terminate())?;
    let auth = match config.mcp_token {
        Some(_) => "bearer token required",
        None => "no auth: mcp_token is not set",
    };
    eprintln!(
        "arc mcp: serving http://{}{MCP_PATH} ({auth})",
        listener.local_addr()?
    );
    serve(listener, config.clone(), binding.extra_hosts, async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    })
    .await
}

pub async fn serve(
    listener: TcpListener,
    config: Config,
    extra_hosts: Vec<String>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let bound = listener.local_addr()?;
    let allowed_hosts = LOOPBACK_HOSTS
        .into_iter()
        .map(str::to_owned)
        .chain([bound.ip().to_string()])
        .chain(extra_hosts);
    let token = config.mcp_token.clone();
    let server = ArcMcp::remote(config);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_allowed_hosts(allowed_hosts),
    );
    let router = Router::new().nest_service(MCP_PATH, service);
    let router = match token {
        Some(token) => router.layer(middleware::from_fn_with_state(token, require_bearer)),
        None => router,
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

fn presented_bearer(headers: &HeaderMap) -> Option<&str> {
    let (scheme, token) = headers.get(AUTHORIZATION)?.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

async fn require_bearer(
    State(token): State<BearerToken>,
    request: Request,
    next: Next,
) -> Response {
    match presented_bearer(request.headers()).is_some_and(|presented| token.matches(presented)) {
        true => next.run(request).await,
        false => (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Bearer")],
            "missing or wrong bearer token\n",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(authorization).unwrap());
        headers
    }

    #[test]
    fn bearer_scheme_is_case_insensitive_and_required() {
        assert_eq!(presented_bearer(&headers("Bearer abc")), Some("abc"));
        assert_eq!(presented_bearer(&headers("bearer abc")), Some("abc"));
        assert_eq!(presented_bearer(&headers("Basic abc")), None);
        assert_eq!(presented_bearer(&headers("abc")), None);
        assert_eq!(presented_bearer(&HeaderMap::new()), None);
    }
}
