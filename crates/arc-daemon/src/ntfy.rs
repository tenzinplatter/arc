use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::Url;
use reqwest::header::AUTHORIZATION;
use tokio::runtime::Handle;
use tracing::warn;

use crate::scheduler::{Notifier, Receipt};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct NtfyNotifier {
    client: reqwest::Client,
    runtime: Handle,
    endpoint: Url,
    bearer: Option<String>,
}

impl NtfyNotifier {
    pub fn new(runtime: Handle, url: &str, topic: &str, token: Option<&str>) -> Result<Self> {
        let mut endpoint = Url::parse(url).with_context(|| format!("invalid ntfy url {url:?}"))?;
        endpoint
            .path_segments_mut()
            .map_err(|()| anyhow!("ntfy url {url:?} cannot carry a topic path"))?
            .pop_if_empty()
            .push(topic);
        Ok(Self {
            client: reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build()?,
            runtime,
            endpoint,
            bearer: token.map(|t| format!("Bearer {t}")),
        })
    }

    fn request(&self, title: &str, body: &str) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(self.endpoint.clone())
            .query(&[("title", title)])
            .body(body.to_owned());
        match &self.bearer {
            Some(bearer) => request.header(AUTHORIZATION, bearer),
            None => request,
        }
    }
}

impl Notifier for NtfyNotifier {
    fn notify(&self, title: &str, body: &str, receipt: Receipt) {
        let request = self.request(title, body);
        let endpoint = self.endpoint.clone();
        self.runtime.spawn(async move {
            match request.send().await.and_then(|r| r.error_for_status()) {
                Ok(_) => receipt.delivered(),
                Err(e) => {
                    warn!("ntfy publish to {endpoint} failed: {e}");
                    receipt.failed();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::test_support::{FakeClock, scheduler};
    use crate::scheduler::{Clock, Outcome, ReminderKey, ScheduledTodo};
    use chrono::{Local, TimeZone};
    use std::collections::HashSet;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, mpsc};
    use tokio::sync::mpsc::UnboundedReceiver;
    use uuid::Uuid;

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

    fn read_request(stream: &std::net::TcpStream) -> CapturedRequest {
        let mut reader = BufReader::new(stream);
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
        CapturedRequest {
            request_line: request_line.trim_end().to_string(),
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }

    fn serve(statuses: Vec<u16>) -> (String, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                tx.send(read_request(&stream)).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nconnection: close\r\ncontent-length: 2\r\n\r\n{{}}"
                )
                .unwrap();
            }
        });
        (url, rx)
    }

    async fn next_outcome(outcomes: &mut UnboundedReceiver<Outcome>) -> Outcome {
        tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
            .await
            .expect("notifier settled its receipt")
            .expect("outcome channel open")
    }

    async fn publish_once(
        notifier: NtfyNotifier,
        title: &str,
        body: &str,
    ) -> Vec<ReminderKey> {
        let clock = Arc::new(FakeClock::new(Local.with_ymd_and_hms(2026, 6, 5, 9, 0, 0).unwrap()));
        let (mut sched, mut outcomes) = scheduler(clock.clone(), Arc::new(notifier), HashSet::new());
        sched.tick();
        sched.upsert(ScheduledTodo {
            id: Uuid::from_u128(1),
            title: title.into(),
            body: body.into(),
            fire_at: clock.now(),
        });
        sched.tick();
        let outcome = next_outcome(&mut outcomes).await;
        sched.settle(outcome)
    }

    #[tokio::test]
    async fn posts_body_to_topic_with_title_query_and_bearer() {
        let (url, requests) = serve(vec![200]);
        let notifier =
            NtfyNotifier::new(Handle::current(), &format!("{url}/"), "arc-test", Some("tk_abc")).unwrap();

        let recorded = publish_once(notifier, "arc — todos", "ship it").await;

        assert_eq!(recorded.len(), 1, "delivered");
        let req = requests.try_recv().unwrap();
        assert_eq!(req.request_line, "POST /arc-test?title=arc+%E2%80%94+todos HTTP/1.1");
        assert_eq!(req.header("title"), None);
        assert_eq!(req.header("authorization"), Some("Bearer tk_abc"));
        assert_eq!(req.body, "ship it");
    }

    #[tokio::test]
    async fn omits_authorization_without_token() {
        let (url, requests) = serve(vec![200]);
        let notifier = NtfyNotifier::new(Handle::current(), &url, "arc-test", None).unwrap();

        let _ = publish_once(notifier, "t", "b").await;

        assert_eq!(requests.try_recv().unwrap().header("authorization"), None);
    }

    #[tokio::test]
    async fn topic_is_percent_encoded_under_the_url_path() {
        let (url, requests) = serve(vec![200]);
        let notifier = NtfyNotifier::new(Handle::current(), &format!("{url}/ntfy/"), "a b/c", None).unwrap();

        let _ = publish_once(notifier, "t", "b").await;

        assert_eq!(requests.try_recv().unwrap().request_line, "POST /ntfy/a%20b%2Fc?title=t HTTP/1.1");
    }

    #[tokio::test]
    async fn invalid_url_fails_at_construction() {
        assert!(NtfyNotifier::new(Handle::current(), "not a url", "arc", None).is_err());
        assert!(NtfyNotifier::new(Handle::current(), "mailto:me@example.com", "arc", None).is_err());
    }

    #[tokio::test]
    async fn rejected_publish_is_retried_and_recorded_once_it_succeeds() {
        let (url, requests) = serve(vec![403, 200]);
        let clock = Arc::new(FakeClock::new(Local.with_ymd_and_hms(2026, 6, 5, 9, 0, 0).unwrap()));
        let notifier = NtfyNotifier::new(Handle::current(), &url, "arc-test", None).unwrap();
        let (mut sched, mut outcomes) = scheduler(clock.clone(), Arc::new(notifier), HashSet::new());
        sched.tick();
        let reminder = ScheduledTodo {
            id: Uuid::from_u128(1),
            title: "t".into(),
            body: "b".into(),
            fire_at: clock.now(),
        };
        sched.upsert(reminder.clone());

        sched.tick();
        let rejected = next_outcome(&mut outcomes).await;
        assert!(sched.settle(rejected).is_empty(), "403 is not recorded as fired");
        assert_eq!(sched.next_fire(), Some(clock.now() + chrono::Duration::minutes(1)));

        clock.advance(chrono::Duration::minutes(1));
        sched.tick();
        let accepted = next_outcome(&mut outcomes).await;
        assert_eq!(sched.settle(accepted), vec![reminder.key()]);
        assert_eq!(requests.try_iter().count(), 2);
        assert_eq!(sched.next_fire(), None);
    }
}
