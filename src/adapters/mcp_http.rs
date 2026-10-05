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
use hyper::header::{ALLOW, CONTENT_TYPE, WWW_AUTHENTICATE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::domain::functions::mcp::{Incoming, ToolRun, result};
use crate::domain::functions::mcp_http::{McpToken, Refusal, RequestHead, admit, read_body};
use crate::domain::types::limits::McpHttpLimits;

/// Runs one tool and answers its MCP tool result.
pub type RunTool =
    Arc<dyn Fn(ToolRun) -> Pin<Box<dyn Future<Output = Value> + Send>> + Send + Sync>;

/// What every request is checked against, and how a tool call runs.
pub struct Endpoint {
    pub token: McpToken,
    pub exposed: Vec<String>,
    pub run_tool: RunTool,
    /// `limits::MCP_HTTP`; a test passes shorter deadlines.
    pub limits: McpHttpLimits,
}

/// Bind `address`; the port is the one asked for, or the one the system
/// picked for port 0.
pub async fn bind(address: SocketAddr) -> Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    Ok((listener, local))
}

/// Answer connections until the process ends. A connection that cannot be
/// accepted (the descriptors are used up, the client reset it first) is that
/// connection's failure, never the server's: anyone who can reach the port
/// could otherwise stop it.
pub async fn serve(listener: TcpListener, endpoint: Arc<Endpoint>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                crate::console::write_line(&format!("accept failed: {error}"));
                // Descriptors used up stay used up for a while: do not spin.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let endpoint = Arc::clone(&endpoint);
        let header_secs = endpoint.limits.header_secs;
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let endpoint = Arc::clone(&endpoint);
                async move { Ok::<_, Infallible>(answer(request, &endpoint).await) }
            });
            // A client that stops sending its headers is dropped; a broken
            // connection is the client's business, not the server's.
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(header_secs))
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
    let version = request
        .headers()
        .get("mcp-protocol-version")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    let body = tokio::time::timeout(
        Duration::from_secs(endpoint.limits.body_secs),
        Limited::new(request.into_body(), endpoint.limits.body_bytes).collect(),
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
    Ok(
        match read_body(&body, version.as_deref(), &endpoint.exposed)? {
            Incoming::Nothing => empty(StatusCode::ACCEPTED),
            Incoming::Answer(answer) => json(StatusCode::OK, &answer),
            Incoming::Call { id, run } => {
                json(StatusCode::OK, &result(id, (endpoint.run_tool)(run).await))
            }
        },
    )
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
        Refusal::MethodNotAllowed => {
            let mut response = empty(status);
            response
                .headers_mut()
                .insert(ALLOW, "POST".parse().expect("a header value"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// A server with one-second deadlines, answering every tool call `{}`.
    async fn server() -> SocketAddr {
        let (listener, address) = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let run_tool: RunTool = Arc::new(|_| Box::pin(async { serde_json::json!({}) }));
        let endpoint = Endpoint {
            token: McpToken::new("t0ken".to_owned()).unwrap(),
            exposed: Vec::new(),
            run_tool,
            limits: McpHttpLimits {
                body_bytes: 1024,
                header_secs: 1,
                body_secs: 1,
            },
        };
        tokio::spawn(serve(listener, Arc::new(endpoint)));
        address
    }

    /// Write `bytes`, then read until the server closes, and how long it took.
    async fn exchange(address: SocketAddr, bytes: &[u8]) -> (String, Duration) {
        let started = Instant::now();
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer)).await;
        (
            String::from_utf8_lossy(&answer).into_owned(),
            started.elapsed(),
        )
    }

    #[tokio::test]
    async fn a_body_that_stops_arriving_is_400_at_the_body_deadline() {
        let address = server().await;
        let (answer, took) = exchange(
            address,
            b"POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t0ken\r\n\
              Content-Type: application/json\r\nContent-Length: 10\r\n\r\n{\"a\"",
        )
        .await;
        assert!(answer.starts_with("HTTP/1.1 400"), "{answer:?}");
        assert!(
            took >= Duration::from_millis(900) && took < Duration::from_secs(4),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn headers_that_never_end_close_the_connection_at_the_header_deadline() {
        let address = server().await;
        let (_, took) = exchange(address, b"POST /mcp HTTP/1.1\r\nHost: x\r\n").await;
        assert!(
            took >= Duration::from_millis(900) && took < Duration::from_secs(4),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn a_method_other_than_post_says_which_one_is_allowed() {
        let address = server().await;
        let (answer, _) = exchange(
            address,
            b"GET /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: t0ken\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(answer.starts_with("HTTP/1.1 405"), "{answer:?}");
        assert!(
            answer.to_ascii_lowercase().contains("\r\nallow: post\r\n"),
            "{answer:?}"
        );
    }
}
