//! `list`: the values of the search forms' select boxes.

use anyhow::Result;
use reqwest::Client;

use crate::cli::NationalList;
use crate::http::fetch_html;
use crate::output::print_json_array;
use crate::parse::{parse_agencies, parse_options, parse_years};
use crate::site::{LAW_TYPES, SEARCH_PATH, SITE_ORIGIN};

pub(crate) async fn run_list(client: &Client) -> Result<()> {
    let html = fetch_html(
        client,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=provincial"),
    )
    .await?;
    print_json_array(
        &parse_options(&html, "select[name=provincia]")?,
        "provinces",
    )
}

async fn fetch_national_form(client: &Client) -> Result<String> {
    fetch_html(
        client,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=nacional"),
    )
    .await
}

/// Lists of the national search form. `LawType` is a local constant (`run` answers it without
/// a request); it is kept here so the match stays exhaustive.
pub(crate) async fn run_list_national(client: &Client, what: NationalList) -> Result<()> {
    match what {
        NationalList::Agencies => print_json_array(
            &parse_agencies(&fetch_national_form(client).await?)?,
            "agencies",
        ),
        NationalList::Years => {
            print_json_array(&parse_years(&fetch_national_form(client).await?)?, "years")
        }
        NationalList::LawType => print_json_array(LAW_TYPES, "law types"),
    }
}
