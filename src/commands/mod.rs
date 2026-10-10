//! One runner per subcommand; each drives an already launched browser page.

mod fetch;
mod list;
mod query;

pub(crate) use fetch::run_fetch;
pub(crate) use list::{run_list, run_list_national};
pub(crate) use query::{run_query_national, run_query_provinces};
