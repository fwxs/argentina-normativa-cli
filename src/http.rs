//! Plain HTTP GETs: every page the scraper reads is server-rendered, so no browser is needed.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use reqwest::redirect::Policy;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_REDIRECTS: usize = 5;

pub(crate) fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::limited(MAX_REDIRECTS))
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
