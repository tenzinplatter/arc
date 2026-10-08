use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use tokio::runtime::Handle;
use tracing::warn;

use crate::scheduler::Notifier;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct NtfyNotifier {
    client: reqwest::Client,
    runtime: Handle,
    endpoint: String,
    bearer: Option<String>,
}

impl NtfyNotifier {
    pub fn new(runtime: Handle, url: &str, topic: &str, token: Option<&str>) -> reqwest::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build()?,
            runtime,
            endpoint: format!("{}/{}", url.trim_end_matches('/'), topic),
            bearer: token.map(|t| format!("Bearer {t}")),
        })
    }

    fn request(&self, title: &str, body: &str) -> reqwest::RequestBuilder {
        let title = HeaderValue::from_bytes(title.as_bytes())
            .unwrap_or_else(|_| HeaderValue::from_static("arc"));
        let request = self
            .client
            .post(&self.endpoint)
            .header("Title", title)
            .body(body.to_owned());
        match &self.bearer {
            Some(bearer) => request.header(AUTHORIZATION, bearer),
            None => request,
        }
    }
}

impl Notifier for NtfyNotifier {
    fn notify(&self, title: &str, body: &str) {
        let request = self.request(title, body);
        let endpoint = self.endpoint.clone();
        self.runtime.spawn(async move {
            if let Err(e) = request.send().await.and_then(|r| r.error_for_status()) {
                warn!("ntfy publish to {endpoint} failed: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    struct CapturedRequest {
        request_line: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl CapturedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    fn capture_one_request() -> (String, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, v)| v.parse().unwrap());
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .unwrap();
            tx.send(CapturedRequest {
                request_line: request_line.trim_end().to_string(),
                headers,
                body: String::from_utf8(body).unwrap(),
            })
            .unwrap();
        });
        (url, rx)
    }

    #[tokio::test]
    async fn posts_body_to_topic_with_title_and_bearer() {
        let (url, rx) = capture_one_request();
        let notifier =
            NtfyNotifier::new(Handle::current(), &format!("{url}/"), "arc-test", Some("tk_abc")).unwrap();

        notifier.notify("arc — todos", "ship it");

        let req = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .expect("ntfy request captured");
        assert_eq!(req.request_line, "POST /arc-test HTTP/1.1");
        assert_eq!(req.header("title"), Some("arc — todos"));
        assert_eq!(req.header("authorization"), Some("Bearer tk_abc"));
        assert_eq!(req.body, "ship it");
    }

    #[tokio::test]
    async fn omits_authorization_without_token() {
        let (url, rx) = capture_one_request();
        let notifier = NtfyNotifier::new(Handle::current(), &url, "arc-test", None).unwrap();

        notifier.notify("t", "b");

        let req = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .expect("ntfy request captured");
        assert_eq!(req.header("authorization"), None);
    }
}
