//! Writers for the data the commands print to stdout.

use std::io::Write;

use anyhow::{Context, Result};
use serde::Serialize;

pub(crate) fn print_json_array<T: Serialize>(items: &[T], what: &str) -> Result<()> {
    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string_pretty(items)?
    )
    .with_context(|| format!("failed to write {what} to stdout"))
}
