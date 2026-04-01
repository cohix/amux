#![allow(dead_code)]

mod cli;
mod commands;
mod config;
mod docker;
mod git;
mod tui;
mod workflow;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(cmd) => commands::run(cmd).await,
        None => {
            let startup_ready_flags = tui::StartupReadyFlags {
                build: cli.build,
                no_cache: cli.no_cache,
                refresh: cli.refresh,
            };
            tui::run(startup_ready_flags).await
        }
    }
}
