//! Plain HTTP GETs: every page the scraper reads is server-rendered, so no browser is needed.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::redirect::{Attempt, Policy};
use reqwest::{Client, Url};

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

/// Body of `url`; a non-2xx status is an error, kept as the `reqwest::Error` so callers can read it.
pub(crate) async fn fetch_html(client: &Client, url: &str) -> Result<String> {
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("failed to open {url}"))?
        .text()
        .await
        .with_context(|| format!("failed to read content of {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
