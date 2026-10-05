use anyhow::Context;
use reqwest::{Client, RequestBuilder, Response};
use serde::Serialize;

pub mod branch;
pub mod epic;
pub mod iteration;
pub mod story;
pub mod user;
pub mod workflow;

use user::CurrentUser;

pub const API_BASE_URL: &str = "https://api.app.shortcut.com/api/v3";

#[derive(Clone)]
pub struct ApiClient {
    api_token: String,
    user: CurrentUser,
    http_client: Client,
}

pub fn get_full_path(endpoint: &str) -> String {
    assert!(!endpoint.starts_with("/"));
    format!("{}/{}", API_BASE_URL, endpoint)
}

impl ApiClient {
    async fn put_with_body<Body>(&self, endpoint: &str, body: &Body) -> anyhow::Result<Response>
    where
        Body: Serialize,
    {
        let full_path = get_full_path(endpoint);
        self.put_request(&full_path)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to PUT {} with body", full_path))
    }

    async fn get_with_body<Body>(&self, endpoint: &str, body: &Body) -> anyhow::Result<Response>
    where
        Body: Serialize,
    {
        let full_path = get_full_path(endpoint);
        self.get_request(&full_path)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to GET {} with body", full_path))
    }

    async fn get(&self, endpoint: &str) -> anyhow::Result<Response> {
        let full_path = get_full_path(endpoint);
        self.get_request(&full_path)
            .send()
            .await
            .with_context(|| format!("Failed to GET {}", full_path))
    }

    fn get_request(&self, path: &str) -> RequestBuilder {
        self.http_client
            .get(path)
            .header("Shortcut-Token", &self.api_token)
            .header("Content-Type", "application/json")
    }

    fn put_request(&self, path: &str) -> RequestBuilder {
        self.http_client
            .put(path)
            .header("Shortcut-Token", &self.api_token)
            .header("Content-Type", "application/json")
    }

    pub fn new(api_token: String, user: CurrentUser) -> Self {
        Self {
            api_token,
            user,
            http_client: Client::new(),
        }
    }

    pub fn user(&self) -> &CurrentUser {
        &self.user
    }

    pub async fn get_with_query(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> anyhow::Result<Response> {
        let full_path = get_full_path(endpoint);
        self.get_request(&full_path)
            .query(query)
            .send()
            .await
            .with_context(|| format!("Failed to GET {}", full_path))
    }

    /// `next_path` is Shortcut's pagination `next`: a URL path and query, e.g. `/api/v3/search/stories?next=TOKEN`.
    pub(crate) async fn get_next_page(&self, next_path: &str) -> anyhow::Result<Response> {
        let url = if next_path.starts_with("http") {
            next_path.to_string()
        } else if let Some(rest) = next_path.strip_prefix("/") {
            format!("https://api.app.shortcut.com/{rest}")
        } else {
            format!("https://api.app.shortcut.com/{next_path}")
        };
        self.get_request(&url)
            .send()
            .await
            .with_context(|| format!("Failed to GET {}", url))
    }
}
