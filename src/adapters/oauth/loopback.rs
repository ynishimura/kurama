//! Loopback listener for the authorization code redirect: one HTTP request
//! to `http://127.0.0.1:<port>/callback`, answered with a page that tells
//! the person to close the window.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::domain::functions::oauth::redirect_uri;

pub struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

const CLOSE_PAGE: &str = "<!doctype html><html><body style=\"font-family: sans-serif\">\
<p>kurama received the authorization. You can close this window.</p></body></html>";

/// How long one connection may take to send its request line and headers;
/// a browser sends them at once, a speculative connection never does.
const HEAD_TIMEOUT: Duration = Duration::from_secs(2);

impl CallbackListener {
    /// Listen on `127.0.0.1:<port>`, or on an ephemeral port when none is given.
    pub async fn bind(port: Option<u16>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(0))).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub fn redirect_uri(&self) -> String {
        redirect_uri(self.port)
    }

    /// The query string of the first request for `/callback`. Other paths
    /// (a favicon) get a 404, and a connection that sends nothing or resets
    /// is dropped; neither ends the wait.
    pub async fn wait(self, limit: Duration) -> Result<String, String> {
        timeout(limit, self.accept_callback())
            .await
            .map_err(|_| {
                format!(
                    "no browser redirect within {}s; run `kurama login` again",
                    limit.as_secs()
                )
            })?
            .map_err(|error| format!("browser redirect failed: {error}"))
    }

    async fn accept_callback(self) -> std::io::Result<String> {
        loop {
            let (mut stream, _) = self.listener.accept().await?;
            // Browsers open speculative connections and drop them; only the
            // request for the redirect matters.
            let Ok(Ok(head)) = timeout(HEAD_TIMEOUT, read_head(&mut stream)).await else {
                continue;
            };
            match request_target(&head) {
                Some(target) if target == "/callback" || target.starts_with("/callback?") => {
                    respond(
                        &mut stream,
                        "200 OK",
                        "text/html; charset=utf-8",
                        CLOSE_PAGE,
                    )
                    .await?;
                    return Ok(target.split_once('?').map_or("", |(_, q)| q).to_string());
                }
                _ => {
                    let _ = respond(&mut stream, "404 Not Found", "text/plain", "not found").await;
                }
            }
        }
    }
}

/// The request line and headers, at most 16 KiB.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") || buffer.len() > 16 * 1024 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// The request target of `GET /callback?code=... HTTP/1.1`.
fn request_target(head: &str) -> Option<&str> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method == "GET").then_some(target)
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn the_callback_query_is_returned_and_other_paths_are_ignored() {
        let listener = CallbackListener::bind(None).await.unwrap();
        let port = listener.port;
        assert_eq!(
            listener.redirect_uri(),
            format!("http://127.0.0.1:{port}/callback")
        );
        let waiting = tokio::spawn(listener.wait(Duration::from_secs(5)));
        let favicon = get(port, "/favicon.ico").await;
        assert!(favicon.starts_with("HTTP/1.1 404"));
        let callback = get(port, "/callback?code=abc&state=s1").await;
        assert!(callback.starts_with("HTTP/1.1 200"));
        assert!(callback.contains("close this window"));
        assert_eq!(waiting.await.unwrap().unwrap(), "code=abc&state=s1");
    }

    #[tokio::test]
    async fn a_connection_that_sends_nothing_does_not_end_the_wait() {
        let listener = CallbackListener::bind(None).await.unwrap();
        let port = listener.port;
        let waiting = tokio::spawn(listener.wait(Duration::from_secs(5)));
        let silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        drop(silent);
        let callback = get(port, "/callback?code=abc&state=s1").await;
        assert!(callback.starts_with("HTTP/1.1 200"));
        assert_eq!(waiting.await.unwrap().unwrap(), "code=abc&state=s1");
    }

    #[tokio::test]
    async fn a_connection_that_stays_silent_does_not_block_the_redirect() {
        let listener = CallbackListener::bind(None).await.unwrap();
        let port = listener.port;
        let waiting = tokio::spawn(listener.wait(Duration::from_secs(10)));
        let _silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let callback = get(port, "/callback?code=abc&state=s1").await;
        assert!(callback.starts_with("HTTP/1.1 200"));
        assert_eq!(waiting.await.unwrap().unwrap(), "code=abc&state=s1");
    }

    #[tokio::test]
    async fn waiting_gives_up_after_the_timeout() {
        let listener = CallbackListener::bind(None).await.unwrap();
        let error = listener.wait(Duration::from_millis(50)).await.unwrap_err();
        assert!(error.contains("no browser redirect"), "{error}");
    }

    #[test]
    fn request_target_needs_a_get_line() {
        assert_eq!(
            request_target("GET /callback?x=1 HTTP/1.1\r\nHost: a\r\n"),
            Some("/callback?x=1")
        );
        assert_eq!(request_target("POST /callback HTTP/1.1\r\n"), None);
        assert_eq!(request_target(""), None);
    }
}
