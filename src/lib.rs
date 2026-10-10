//! Scrapes "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
//! `list` prints the provinces of the search form; `query provinces` prints matching provincial laws and
//! `query national` national norms, as JSON lines;
//! `fetch` prints one law's details as JSON and saves its text as a PDF.

mod browser;
mod cli;
mod commands;
mod model;
mod output;
mod parse;
mod site;
mod validate;

use anyhow::{Context, Result};

use crate::browser::launch_browser;
pub use crate::cli::{
    Cli, Command, ListScope, NationalArgs, NationalFilters, NationalList, ProvinceArgs, QueryScope,
};
use crate::commands::{
    run_fetch, run_list, run_list_national, run_query_national, run_query_provinces,
};
use crate::output::print_json_array;
use crate::site::LAW_TYPES;
use crate::validate::{validate_fetch, validate_national};

/// Runs one parsed command: validates input, drives Chrome and prints the result to stdout.
///
/// Validation happens before Chrome starts so `--help` and bad arguments never launch a browser.
pub async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Fetch {
            jurisdiction, law, ..
        } => validate_fetch(*jurisdiction, law)?,
        Command::Query {
            scope: QueryScope::National(args),
        } => validate_national(&args.filters)?,
        // Slugs are a local constant: answer without launching Chrome.
        Command::List {
            scope:
                Some(ListScope::National {
                    what: NationalList::LawType,
                }),
        } => return print_json_array(LAW_TYPES, "law types"),
        Command::Query {
            scope: QueryScope::Provinces(_),
        }
        | Command::List { .. } => {}
    }

    let (mut browser, handler_task, page) = launch_browser().await?;
    let outcome = match &cli.command {
        Command::List { scope: None } => run_list(&page).await,
        Command::List {
            scope: Some(ListScope::National { what }),
        } => run_list_national(&page, *what).await,
        Command::Query {
            scope: QueryScope::Provinces(args),
        } => run_query_provinces(&page, args).await,
        Command::Query {
            scope: QueryScope::National(args),
        } => run_query_national(&page, args).await,
        Command::Fetch {
            jurisdiction,
            law,
            output,
        } => run_fetch(&page, *jurisdiction, law, output.as_deref()).await,
    };
    // Close before propagating the command error so Chrome never outlives us.
    browser.close().await.context("failed to close browser")?;
    handler_task
        .await
        .context("browser handler task panicked")?;
    outcome
}
