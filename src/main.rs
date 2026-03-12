mod cli;
mod commands;
mod config;
mod docker;
mod tui;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(cmd) => commands::run(cmd).await,
        None => tui::run().await,
    }
}
