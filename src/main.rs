use anyhow::Result;
use argentina_normativa_cli::{Cli, run};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse first so `--help` and argument errors never send a request.
    let cli = Cli::parse();

    // stdout carries data only; logs go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    run(cli).await
}
