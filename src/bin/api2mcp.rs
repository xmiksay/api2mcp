//! Thin entry point. Everything of substance lives in the library so tests can drive it.

use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    api2mcp::observe::init_tracing();
    let cli = api2mcp::cli::Cli::parse();
    match api2mcp::cli::run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // `{err:#}` renders the whole anyhow context chain on one line.
            tracing::error!("{err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
