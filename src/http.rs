//! Plain HTTP GETs: every page the scraper reads is server-rendered, so no browser is needed.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::RETRY_AFTER;
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, Response, StatusCode, Url};

use crate::site::{CRAWL_DELAY, SITE_HOST};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_REDIRECTS: usize = 5;

// Total tries per page, the first one included.
const MAX_ATTEMPTS: u32 = 3;

// Upper bound of one wait, so a huge `Retry-After` can't stall the command.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

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
    Status {
        url: String,
        status: StatusCode,
        /// Seconds the site asked to wait before trying again, if it said.
        retry_after: Option<Duration>,
    },
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

/// `Retry-After` as delta-seconds; the HTTP-date form is ignored.
fn retry_after(response: &Response) -> Option<Duration> {
    let seconds = response.headers().get(RETRY_AFTER)?.to_str().ok()?;
    seconds.trim().parse().ok().map(Duration::from_secs)
}

/// How long to wait before the next try after `attempt` (1-based) failed with `error`, or `None`
/// when the failure is not transient or the tries are used up.
///
/// The wait is never shorter than the crawl delay, doubles per attempt and is capped.
fn retry_delay(error: &FetchError, attempt: u32) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let site_wait = match error {
        FetchError::Status {
            status,
            retry_after,
            ..
        } if matches!(
            *status,
            StatusCode::TOO_MANY_REQUESTS
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        ) =>
        {
            retry_after.unwrap_or_default()
        }
        FetchError::Request { source, .. } | FetchError::Body { source, .. }
            if source.is_timeout() || source.is_connect() =>
        {
            Duration::ZERO
        }
        FetchError::Status { .. } | FetchError::Request { .. } | FetchError::Body { .. } => {
            return None;
        }
    };
    let backoff = site_wait.max(CRAWL_DELAY) * 2_u32.pow(attempt - 1);
    Some(backoff.min(MAX_RETRY_WAIT))
}

async fn fetch_once(client: &Client, url: &str) -> Result<String, FetchError> {
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
            retry_after: retry_after(&response),
        });
    }
    response.text().await.map_err(|source| FetchError::Body {
        url: url.to_owned(),
        source,
    })
}

/// Body of `url`; a non-2xx status is a `FetchError::Status`. Rate limiting, gateway errors and
/// timeouts are retried a couple of times, one request at a time, after at least the crawl delay.
pub(crate) async fn fetch_html(client: &Client, url: &str) -> Result<String, FetchError> {
    let mut attempt = 1;
    loop {
        let error = match fetch_once(client, url).await {
            Ok(html) => return Ok(html),
            Err(error) => error,
        };
        let Some(delay) = retry_delay(&error, attempt) else {
            return Err(error);
        };
        tracing::warn!(
            attempt,
            wait_seconds = delay.as_secs(),
            %error,
            "request failed, retrying"
        );
        tokio::time::sleep(delay).await;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Answers consecutive connections with the canned `responses` and returns the url to request.
    async fn serve(responses: Vec<&'static str>) -> Result<String> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        tokio::spawn(async move {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok(url)
    }

    fn status_error(status: StatusCode, retry_after: Option<Duration>) -> FetchError {
        FetchError::Status {
            url: "https://example.com/".to_owned(),
            status,
            retry_after,
        }
    }

    const SERVICE_UNAVAILABLE: &str =
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n";

    fn test_client() -> Result<Client> {
        Ok(Client::builder().no_proxy().build()?)
    }

    #[tokio::test]
    async fn fetch_html_not_found_returns_typed_not_found_status() -> Result<()> {
        // Arrange
        let url = serve(vec!["HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n"]).await?;

        // Act
        let result = fetch_html(&test_client()?, &url).await;

        // Assert
        assert!(
            matches!(&result, Err(error) if error.is_not_found()),
            "{result:?}"
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_html_persistent_server_error_gives_up_with_status() -> Result<()> {
        // Arrange
        let url = serve(vec![SERVICE_UNAVAILABLE; MAX_ATTEMPTS as usize]).await?;

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

    #[tokio::test(start_paused = true)]
    async fn fetch_html_transient_error_then_ok_returns_body() -> Result<()> {
        // Arrange
        let url = serve(vec![
            SERVICE_UNAVAILABLE,
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi",
        ])
        .await?;

        // Act
        let body = fetch_html(&test_client()?, &url).await?;

        // Assert
        assert_eq!(body, "hi");
        Ok(())
    }

    #[tokio::test]
    async fn fetch_html_ok_response_returns_body() -> Result<()> {
        // Arrange
        let url = serve(vec!["HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi"]).await?;

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

    #[test]
    fn retry_delay_transient_status_waits_at_least_the_crawl_delay_and_doubles() {
        // Act / Assert
        let error = status_error(StatusCode::SERVICE_UNAVAILABLE, None);
        assert_eq!(retry_delay(&error, 1), Some(CRAWL_DELAY));
        assert_eq!(retry_delay(&error, 2), Some(CRAWL_DELAY * 2));
    }

    #[test]
    fn retry_delay_attempts_used_up_returns_none() {
        // Act / Assert
        let error = status_error(StatusCode::TOO_MANY_REQUESTS, None);
        assert_eq!(retry_delay(&error, MAX_ATTEMPTS), None);
    }

    #[test]
    fn retry_delay_retry_after_raises_the_wait_up_to_the_cap() {
        // Act / Assert
        let asked = |seconds| status_error(StatusCode::TOO_MANY_REQUESTS, Some(seconds));
        assert_eq!(
            retry_delay(&asked(Duration::from_secs(30)), 1),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_delay(&asked(Duration::from_secs(2)), 1),
            Some(CRAWL_DELAY)
        );
        assert_eq!(
            retry_delay(&asked(Duration::from_secs(600)), 1),
            Some(MAX_RETRY_WAIT)
        );
    }

    #[test]
    fn retry_delay_permanent_statuses_return_none() {
        // Act / Assert
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert_eq!(
                retry_delay(&status_error(status, None), 1),
                None,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn retry_delay_connection_refused_is_transient() -> Result<()> {
        // Arrange: a port that was just released refuses connections.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        drop(listener);

        // Act
        let Err(error) = fetch_once(&test_client()?, &url).await else {
            anyhow::bail!("connection to a closed port succeeded");
        };

        // Assert
        assert_eq!(retry_delay(&error, 1), Some(CRAWL_DELAY));
        Ok(())
    }
}
