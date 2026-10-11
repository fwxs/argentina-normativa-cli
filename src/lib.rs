//! Scrapes "Normativas" from argentina.gob.ar/normativa with plain HTTP requests.
//! `list` prints the provinces of the search form; `query provinces` prints matching provincial laws and
//! `query national` national norms, as JSON lines;
//! `fetch` prints one law's details as JSON and saves its text as Markdown.

mod cli;
mod commands;
mod http;
mod model;
mod output;
mod parse;
mod site;
mod validate;

use anyhow::Result;

pub use crate::cli::{
    Cli, Command, ListScope, NationalArgs, NationalFilters, NationalList, ProvinceArgs, QueryScope,
};
use crate::commands::{
    run_fetch, run_list, run_list_national, run_query_national, run_query_provinces,
};
use crate::http::build_client;
use crate::output::print_json_array;
use crate::site::LAW_TYPES;
use crate::validate::{validate_fetch, validate_national, validate_provinces};

/// Runs one parsed command: validates input, fetches the pages and prints the result to stdout.
///
/// Validation happens first so bad arguments never send a request.
pub async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Fetch {
            jurisdiction,
            law,
            output,
        } => validate_fetch(*jurisdiction, law, output.as_deref())?,
        Command::Query {
            scope: QueryScope::National(args),
        } => validate_national(&args.filters)?,
        // Slugs are a local constant: answer without a request.
        Command::List {
            scope:
                Some(ListScope::National {
                    what: NationalList::LawType,
                }),
        } => return print_json_array(LAW_TYPES, "law types"),
        Command::Query {
            scope: QueryScope::Provinces(args),
        } => validate_provinces(args)?,
        Command::List { .. } => {}
    }

    let client = build_client()?;
    match &cli.command {
        Command::List { scope: None } => run_list(&client).await,
        Command::List {
            scope: Some(ListScope::National { what }),
        } => run_list_national(&client, *what).await,
        Command::Query {
            scope: QueryScope::Provinces(args),
        } => run_query_provinces(&client, args).await,
        Command::Query {
            scope: QueryScope::National(args),
        } => run_query_national(&client, args).await,
        Command::Fetch {
            jurisdiction,
            law,
            output,
        } => run_fetch(&client, *jurisdiction, law, output.as_deref()).await,
    }
}
