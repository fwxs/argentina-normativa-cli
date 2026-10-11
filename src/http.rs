//! Plain HTTP GETs: every page the scraper reads is server-rendered, so no browser is needed.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, StatusCode, Url};

use crate::site::SITE_HOST;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_REDIRECTS: usize = 5;

/// Whether a redirect may be followed: only to `https` on the site's own host, a bounded number
/// of times, so a `Location` header can't send a request elsewhere or downgrade it to plain HTTP.
fn is_allowed_redirect(target: &Url, redirects_so_far: usize) -> bool {
    redirects_so_far < MAX_REDIRECTS
        && target.scheme() == "https"
        && target.host_str() == Some(SITE_HOST)
        && target.username().is_empty()
        && target.password().is_none()
}

// A refused redirect is an error: stopping would hand back the 3xx response, which
// `error_for_status` does not reject, and its body would be parsed as the page.
fn follow_site_redirects(attempt: Attempt) -> reqwest::redirect::Action {
    if is_allowed_redirect(attempt.url(), attempt.previous().len()) {
        attempt.follow()
    } else {
        let message = format!("refusing to follow redirect to {}", attempt.url());
        attempt.error(message)
    }
}

pub(crate) fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::custom(follow_site_redirects))
        .build()
        .context("failed to build the HTTP client")
}

/// Why a page could not be fetched; the variants let callers react to a status without guessing
/// from error text.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("failed to open {url}: the site answered {status}")]
    Status { url: String, status: StatusCode },
    #[error("failed to open {url}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("failed to read content of {url}")]
    Body {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

impl FetchError {
    pub(crate) fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Status {
                status: StatusCode::NOT_FOUND,
                ..
            }
        )
    }
}

/// Body of `url`; a non-2xx status is a `FetchError::Status`.
pub(crate) async fn fetch_html(client: &Client, url: &str) -> Result<String, FetchError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|source| FetchError::Request {
            url: url.to_owned(),
            source,
        })?;
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        return Err(FetchError::Status {
            url: url.to_owned(),
            status,
        });
    }
    response.text().await.map_err(|source| FetchError::Body {
        url: url.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Serves one canned response on a local port and returns the url to request.
    async fn serve_once(response: &'static str) -> Result<String> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok(url)
    }

    fn test_client() -> Result<Client> {
        Ok(Client::builder().no_proxy().build()?)
    }

    #[tokio::test]
    async fn fetch_html_not_found_returns_typed_not_found_status() -> Result<()> {
        // Arrange
        let url = serve_once("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await?;

        // Act
        let result = fetch_html(&test_client()?, &url).await;

        // Assert
        assert!(
            matches!(&result, Err(error) if error.is_not_found()),
            "{result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn fetch_html_server_error_is_status_but_not_not_found() -> Result<()> {
        // Arrange
        let url =
            serve_once("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n").await?;

        // Act
        let result = fetch_html(&test_client()?, &url).await;

        // Assert
        assert!(matches!(
            &result,
            Err(error @ FetchError::Status { status, .. })
                if *status == StatusCode::SERVICE_UNAVAILABLE && !error.is_not_found()
        ));
        Ok(())
    }

    #[tokio::test]
    async fn fetch_html_ok_response_returns_body() -> Result<()> {
        // Arrange
        let url = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi").await?;

        // Act
        let body = fetch_html(&test_client()?, &url).await?;

        // Assert
        assert_eq!(body, "hi");
        Ok(())
    }

    fn is_allowed(target: &str, redirects_so_far: usize) -> bool {
        Url::parse(target).is_ok_and(|url| is_allowed_redirect(&url, redirects_so_far))
    }

    #[test]
    fn is_allowed_redirect_same_site_https_is_followed() {
        // Act / Assert
        assert!(is_allowed(
            "https://www.argentina.gob.ar/normativa/provincial/ley-1",
            0
        ));
        assert!(is_allowed(
            "https://www.argentina.gob.ar/normativa",
            MAX_REDIRECTS - 1
        ));
    }

    #[test]
    fn is_allowed_redirect_http_downgrade_is_refused() {
        // Act / Assert
        assert!(!is_allowed("http://www.argentina.gob.ar/normativa", 0));
    }

    #[test]
    fn is_allowed_redirect_other_host_is_refused() {
        // Act / Assert
        assert!(!is_allowed("https://example.com/normativa", 0));
        assert!(!is_allowed("https://argentina.gob.ar/normativa", 0));
        assert!(!is_allowed("https://www.argentina.gob.ar.example.com/", 0));
    }

    #[test]
    fn is_allowed_redirect_userinfo_trick_is_refused() {
        // Act / Assert
        assert!(!is_allowed("https://www.argentina.gob.ar@example.com/", 0));
        assert!(!is_allowed("https://user:pass@www.argentina.gob.ar/", 0));
    }

    #[test]
    fn is_allowed_redirect_too_many_hops_is_refused() {
        // Act / Assert
        assert!(!is_allowed(
            "https://www.argentina.gob.ar/normativa",
            MAX_REDIRECTS
        ));
    }
}
