//! `list`: the values of the search forms' select boxes.

use anyhow::Result;
use chromiumoxide::page::Page;

use crate::browser::fetch_html;
use crate::cli::NationalList;
use crate::output::print_json_array;
use crate::parse::{parse_agencies, parse_options, parse_years};
use crate::site::{LAW_TYPES, SEARCH_PATH, SITE_ORIGIN};

pub(crate) async fn run_list(page: &Page) -> Result<()> {
    let html = fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=provincial"),
    )
    .await?;
    print_json_array(
        &parse_options(&html, "select[name=provincia]")?,
        "provinces",
    )
}

async fn fetch_national_form(page: &Page) -> Result<String> {
    fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=nacional"),
    )
    .await
}

/// Lists of the national search form. `LawType` is a local constant (`main` answers it before
/// launching Chrome); it is kept here so the match stays exhaustive.
pub(crate) async fn run_list_national(page: &Page, what: NationalList) -> Result<()> {
    match what {
        NationalList::Agencies => print_json_array(
            &parse_agencies(&fetch_national_form(page).await?)?,
            "agencies",
        ),
        NationalList::Years => {
            print_json_array(&parse_years(&fetch_national_form(page).await?)?, "years")
        }
        NationalList::LawType => print_json_array(LAW_TYPES, "law types"),
    }
}
