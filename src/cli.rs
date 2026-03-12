use clap::{Parser, Subcommand, ValueEnum};

/// Manage predictable and secure agentic coding environments.
#[derive(Parser)]
#[command(name = "aspec", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize the current Git repo for use with aspec.
    Init {
        /// Code agent to install in the Dockerfile.dev container.
        #[arg(long, value_enum, default_value = "claude")]
        agent: Agent,
    },

    /// Check Docker daemon, verify Dockerfile.dev, build image, and report status.
    Ready,

    /// Launch the dev container to implement a work item.
    Implement {
        /// Number of the work item to implement (e.g. 0001).
        work_item: u32,
    },
}

#[derive(Clone, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
}

impl Agent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Opencode => "opencode",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn no_args_gives_no_subcommand() {
        let cli = parse(&["aspec"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn init_default_agent_is_claude() {
        let cli = parse(&["aspec", "init"]);
        match cli.command.unwrap() {
            Command::Init { agent } => assert_eq!(agent.as_str(), "claude"),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_explicit_agent() {
        let cli = parse(&["aspec", "init", "--agent", "codex"]);
        match cli.command.unwrap() {
            Command::Init { agent } => assert_eq!(agent.as_str(), "codex"),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn implement_parses_work_item_number() {
        let cli = parse(&["aspec", "implement", "42"]);
        match cli.command.unwrap() {
            Command::Implement { work_item } => assert_eq!(work_item, 42),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn ready_subcommand_parsed() {
        let cli = parse(&["aspec", "ready"]);
        assert!(matches!(cli.command.unwrap(), Command::Ready));
    }
}
