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
pub mod specs;
pub mod status;

use crate::cli::{Command, SpecsAction};
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
            workflow,
            worktree,
            mount_ssh,
        } => implement::run(&work_item, non_interactive, plan, allow_docker, workflow.as_deref(), worktree, mount_ssh).await,
        Command::Chat { non_interactive, plan, allow_docker, mount_ssh } => {
            chat::run(non_interactive, plan, allow_docker, mount_ssh).await
        }
        Command::Claws { action } => claws::run(action).await,
        Command::Status { watch } => status::run(watch).await,
        Command::Specs { action } => match action {
            SpecsAction::New { interview } => specs::run_new(interview).await,
            SpecsAction::Amend { work_item, non_interactive, allow_docker } => {
                specs::run_amend(&work_item, non_interactive, allow_docker).await
            }
        },
    }
}
