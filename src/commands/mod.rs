pub mod agent;
pub mod auth;
pub mod chat;
pub mod claws;
pub mod download;
pub mod implement;
pub mod init;
pub mod new;
pub mod output;
pub mod ready;

use crate::cli::Command;
use anyhow::Result;

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Init { agent, aspec } => init::run(agent, aspec).await,
        Command::Ready {
            refresh,
            build,
            no_cache,
            non_interactive,
            allow_docker,
        } => ready::run(refresh, build, no_cache, non_interactive, allow_docker).await,
        Command::Implement {
            work_item,
            non_interactive,
            plan,
            allow_docker,
        } => implement::run(&work_item, non_interactive, plan, allow_docker).await,
        Command::Chat { non_interactive, plan, allow_docker } => {
            chat::run(non_interactive, plan, allow_docker).await
        }
        Command::New => new::run().await,
        Command::Claws { action } => claws::run(action).await,
    }
}
