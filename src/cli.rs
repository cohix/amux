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
    Ready {
        /// Read the agent API key from host environment variables instead of the system keychain.
        #[arg(long)]
        auth_from_env: bool,

        /// Run the Dockerfile agent audit (skipped by default).
        #[arg(long)]
        refresh: bool,

        /// Run the agent in non-interactive (print) mode instead of interactive mode.
        #[arg(long)]
        non_interactive: bool,
    },

    /// Launch the dev container to implement a work item.
    Implement {
        /// Work item number (e.g. 0001).
        work_item: String,

        /// Read the agent API key from host environment variables instead of the system keychain.
        #[arg(long)]
        auth_from_env: bool,

        /// Run the agent in non-interactive (print) mode instead of interactive mode.
        #[arg(long)]
        non_interactive: bool,
    },

    /// Create a new work item from the template.
    New,
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
            Command::Implement { work_item, .. } => assert_eq!(work_item, "42"),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_parses_four_digit_work_item() {
        let cli = parse(&["aspec", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { work_item, .. } => assert_eq!(work_item, "0001"),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn ready_subcommand_parsed() {
        let cli = parse(&["aspec", "ready"]);
        assert!(matches!(cli.command.unwrap(), Command::Ready { .. }));
    }

    #[test]
    fn ready_auth_from_env_flag() {
        let cli = parse(&["aspec", "ready", "--auth-from-env"]);
        match cli.command.unwrap() {
            Command::Ready { auth_from_env, .. } => assert!(auth_from_env),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_refresh_flag() {
        let cli = parse(&["aspec", "ready", "--refresh"]);
        match cli.command.unwrap() {
            Command::Ready { refresh, .. } => assert!(refresh),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_non_interactive_flag() {
        let cli = parse(&["aspec", "ready", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Ready { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_all_flags() {
        let cli = parse(&["aspec", "ready", "--auth-from-env", "--refresh", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Ready { auth_from_env, refresh, non_interactive } => {
                assert!(auth_from_env);
                assert!(refresh);
                assert!(non_interactive);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_defaults_no_refresh_no_non_interactive() {
        let cli = parse(&["aspec", "ready"]);
        match cli.command.unwrap() {
            Command::Ready { refresh, non_interactive, .. } => {
                assert!(!refresh);
                assert!(!non_interactive);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn implement_auth_from_env_flag() {
        let cli = parse(&["aspec", "implement", "0001", "--auth-from-env"]);
        match cli.command.unwrap() {
            Command::Implement {
                work_item,
                auth_from_env,
                ..
            } => {
                assert_eq!(work_item, "0001");
                assert!(auth_from_env);
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_non_interactive_flag() {
        let cli = parse(&["aspec", "implement", "0001", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Implement { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_defaults_interactive() {
        let cli = parse(&["aspec", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { non_interactive, .. } => assert!(!non_interactive),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn new_subcommand_parsed() {
        let cli = parse(&["aspec", "new"]);
        assert!(matches!(cli.command.unwrap(), Command::New));
    }
}
