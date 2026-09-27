//! `HttpClient` on reqwest: one client per process, a request timeout, no
//! redirects (a redirect would carry the `Authorization` header elsewhere).

use std::time::Duration;

use async_trait::async_trait;

use crate::adapters::utils::error_chain::causes;
use crate::ports::{HttpClient, HttpError, HttpRequest, HttpResponse};

pub struct ReqwestHttpClient {
    client: reqwest::Client,
    timeout: Duration,
}

impl ReqwestHttpClient {
    /// `accept_invalid_certs` is `kurama api -k`; never set it for OAuth
    /// token requests.
    pub fn new(timeout: Duration, accept_invalid_certs: bool) -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(accept_invalid_certs)
            .user_agent(concat!("kurama/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| HttpError::Other(format!("HTTP client: {error}")))?;
        Ok(Self { client, timeout })
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| HttpError::Other(format!("invalid HTTP method {:?}", request.method)))?;
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| classify(error, self.timeout))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| classify(error, self.timeout))?
            .to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// The message names the URL without its query: a `query` source's
/// credential is part of it.
fn classify(mut error: reqwest::Error, timeout: Duration) -> HttpError {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    if error.is_timeout() {
        return HttpError::Timeout {
            seconds: timeout.as_secs(),
        };
    }
    let message = causes(&error);
    if error.is_connect() {
        HttpError::Connect(message)
    } else {
        HttpError::Other(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A redirect to another origin comes back as the 302 itself: following
    /// it would send the API's `Authorization` to a host it was not meant
    /// for.
    #[tokio::test]
    async fn a_redirect_is_returned_and_never_followed() {
        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&elsewhere)
            .await;
        let api = MockServer::start().await;
        let location = format!("{}/stolen", elsewhere.uri());
        Mock::given(any())
            .respond_with(ResponseTemplate::new(302).insert_header("Location", location.as_str()))
            .mount(&api)
            .await;

        let client = ReqwestHttpClient::new(Duration::from_secs(5), false).unwrap();
        let response = client
            .send(
                HttpRequest::new("GET", format!("{}/user", api.uri()))
                    .with_header("Authorization", "Bearer api-token"),
            )
            .await
            .unwrap();

        assert_eq!(response.status, 302);
        assert_eq!(response.header("location"), Some(location.as_str()));
        let received = api.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0].headers.get("authorization").unwrap(),
            "Bearer api-token"
        );
        assert!(
            elsewhere.received_requests().await.unwrap().is_empty(),
            "the redirect target was called"
        );
    }

    /// reqwest names the URL in its error, and a `query` source's credential
    /// is part of the URL: the message must not carry it.
    #[tokio::test]
    async fn a_failed_request_names_its_url_without_the_query() {
        let client = ReqwestHttpClient::new(Duration::from_secs(5), false).unwrap();
        // Port 1 on loopback: refused at once, no server involved.
        let error = client
            .send(HttpRequest::new(
                "GET",
                "http://127.0.0.1:1/api/v2/users/myself?apiKey=s3cret-key",
            ))
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(matches!(error, HttpError::Connect(_)), "{message}");
        assert!(!message.contains("s3cret-key"), "{message}");
        // Which endpoint failed is still said.
        assert!(
            message.contains("http://127.0.0.1:1/api/v2/users/myself"),
            "{message}"
        );
    }
}
