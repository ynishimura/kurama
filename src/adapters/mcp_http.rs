//! `kurama mcp --listen`: the HTTP/1.1 listener (hyper), its deadlines and the bounded body read; what each request is answered with is `domain::functions::mcp_http`.
//!
//! One line per request goes to stderr with the method, the path and the
//! status, never a header or the body: the token travels in a header.

use std::convert::Infallible;
use std::future::Future;
use std::io::Result;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming as Body};
use hyper::header::{CONTENT_TYPE, WWW_AUTHENTICATE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::domain::functions::mcp::{Incoming, ToolRun, result};
use crate::domain::functions::mcp_http::{McpToken, Refusal, RequestHead, admit, read_body};
use crate::domain::types::limits::MCP_HTTP;

/// Runs one tool and answers its MCP tool result.
pub type RunTool =
    Arc<dyn Fn(ToolRun) -> Pin<Box<dyn Future<Output = Value> + Send>> + Send + Sync>;

/// What every request is checked against, and how a tool call runs.
pub struct Endpoint {
    pub token: McpToken,
    pub exposed: Vec<String>,
    pub run_tool: RunTool,
}

/// Bind `address`; the port is the one asked for, or the one the system
/// picked for port 0.
pub async fn bind(address: SocketAddr) -> Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    Ok((listener, local))
}

/// Answer connections until the process ends.
pub async fn serve(listener: TcpListener, endpoint: Arc<Endpoint>) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let endpoint = Arc::clone(&endpoint);
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let endpoint = Arc::clone(&endpoint);
                async move { Ok::<_, Infallible>(answer(request, &endpoint).await) }
            });
            // A client that stops sending its headers is dropped; a broken
            // connection is the client's business, not the server's.
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(MCP_HTTP.header_secs))
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn answer(request: Request<Body>, endpoint: &Endpoint) -> Response<Full<Bytes>> {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let response = match exchange(request, endpoint).await {
        Ok(response) => response,
        Err(refusal) => refused(&refusal),
    };
    crate::console::write_line(&format!("{method} {path} {}", response.status().as_u16()));
    response
}

async fn exchange(
    request: Request<Body>,
    endpoint: &Endpoint,
) -> std::result::Result<Response<Full<Bytes>>, Refusal> {
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    admit(
        &RequestHead {
            method: request.method().as_str(),
            path: request.uri().path(),
            headers: &headers,
        },
        &endpoint.token,
    )?;
    let body = tokio::time::timeout(
        Duration::from_secs(MCP_HTTP.body_secs),
        Limited::new(request.into_body(), MCP_HTTP.body_bytes).collect(),
    )
    .await
    .map_err(|_| Refusal::BadRequest(None))?
    .map_err(
        |error| match error.downcast_ref::<http_body_util::LengthLimitError>() {
            Some(_) => Refusal::PayloadTooLarge,
            None => Refusal::BadRequest(None),
        },
    )?
    .to_bytes();
    Ok(match read_body(&body, &endpoint.exposed)? {
        Incoming::Nothing => empty(StatusCode::ACCEPTED),
        Incoming::Answer(answer) => json(StatusCode::OK, &answer),
        Incoming::Call { id, run } => {
            json(StatusCode::OK, &result(id, (endpoint.run_tool)(run).await))
        }
    })
}

fn refused(refusal: &Refusal) -> Response<Full<Bytes>> {
    let status = StatusCode::from_u16(refusal.status()).expect("a refusal is an HTTP status");
    match refusal {
        Refusal::Unauthorized => {
            let mut response = empty(status);
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, "Bearer".parse().expect("a header value"));
            response
        }
        Refusal::BadRequest(Some(error)) => json(status, error),
        _ => empty(status),
    }
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn json(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        "application/json".parse().expect("a header value"),
    );
    response
}
