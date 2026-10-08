use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
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
use tower::limit::ConcurrencyLimitLayer;
use tower_http::limit::RequestBodyLimitLayer;

use crate::ArcMcp;

pub const MCP_PATH: &str = "/mcp";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 16;
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http(HttpBinding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    Bearer(BearerToken),
    LoopbackOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpBinding {
    addr: SocketAddr,
    extra_hosts: Vec<String>,
    auth: Auth,
}

impl HttpBinding {
    pub fn new(
        addr: SocketAddr,
        extra_hosts: Vec<String>,
        token: Option<BearerToken>,
    ) -> Result<Self> {
        let auth = match token {
            Some(token) => Auth::Bearer(token),
            None if addr.ip().is_loopback() => Auth::LoopbackOnly,
            None => bail!(
                "refusing to serve MCP on non-loopback address {addr} without auth; set mcp_token in the arc config"
            ),
        };
        Ok(Self {
            addr,
            extra_hosts,
            auth,
        })
    }

    pub async fn listen(self) -> Result<HttpListener> {
        let listener = TcpListener::bind(self.addr)
            .await
            .with_context(|| format!("couldn't bind {}", self.addr))?;
        Ok(HttpListener {
            listener,
            extra_hosts: self.extra_hosts,
            auth: self.auth,
        })
    }
}

pub struct HttpListener {
    listener: TcpListener,
    extra_hosts: Vec<String>,
    auth: Auth,
}

impl HttpListener {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }
}

pub async fn run(config: &Config, binding: HttpBinding) -> Result<()> {
    arc_index::open(&config.cache_dir)?;
    let listener = binding.listen().await?;
    let mut terminate = signal(SignalKind::terminate())?;
    let auth = match listener.auth {
        Auth::Bearer(_) => "bearer token required",
        Auth::LoopbackOnly => "loopback only, no token",
    };
    eprintln!(
        "arc mcp: serving http://{}{MCP_PATH} ({auth})",
        listener.local_addr()?
    );
    serve(listener, config.clone(), async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    })
    .await
}

pub async fn serve(
    HttpListener {
        listener,
        extra_hosts,
        auth,
    }: HttpListener,
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let bound = listener.local_addr()?;
    let allowed_hosts = LOOPBACK_HOSTS
        .into_iter()
        .map(str::to_owned)
        .chain([bound.ip().to_string()])
        .chain(extra_hosts);
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
    let router = Router::new()
        .nest_service(MCP_PATH, service)
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BYTES))
        .layer(ConcurrencyLimitLayer::new(MAX_CONCURRENT_REQUESTS));
    let router = match auth {
        Auth::Bearer(token) => router.layer(middleware::from_fn_with_state(token, require_bearer)),
        Auth::LoopbackOnly => router,
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

    fn binding(addr: &str, token: Option<&str>) -> Result<HttpBinding> {
        HttpBinding::new(
            addr.parse().unwrap(),
            vec![],
            token.map(|t| BearerToken::parse(t).unwrap()),
        )
    }

    #[test]
    fn only_loopback_bindings_may_skip_the_token() {
        assert_eq!(
            binding("127.0.0.1:0", None).unwrap().auth,
            Auth::LoopbackOnly
        );
        assert_eq!(
            binding("[::1]:0", None).unwrap().auth,
            Auth::LoopbackOnly
        );
        let err = binding("100.114.114.38:8765", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("mcp_token"), "{err}");
        assert!(binding("0.0.0.0:8765", None).is_err());
        assert!(matches!(
            binding("100.114.114.38:8765", Some("t")).unwrap().auth,
            Auth::Bearer(_)
        ));
    }
}
