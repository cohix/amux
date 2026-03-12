pub mod auth;
pub mod implement;
pub mod init;
pub mod output;
pub mod ready;

use crate::cli::Command;
use anyhow::Result;

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Init { agent } => init::run(agent).await,
        Command::Ready => ready::run().await,
        Command::Implement { work_item } => implement::run(work_item).await,
    }
}
