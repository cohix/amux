use clap::{Parser, Subcommand, ValueEnum};

/// Manage predictable and secure agentic coding environments.
#[derive(Parser)]
#[command(name = "aspec", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Force rebuild the dev container image from Dockerfile.dev (passed to ready on TUI startup).
    #[arg(long, global = true)]
    pub build: bool,

    /// Pass --no-cache to docker build (passed to ready on TUI startup).
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Run the Dockerfile agent audit (passed to ready on TUI startup).
    #[arg(long, global = true)]
    pub refresh: bool,
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
        /// Run the Dockerfile agent audit (skipped by default).
        #[arg(long)]
        refresh: bool,

        /// Force rebuild the dev container image from Dockerfile.dev.
        #[arg(long)]
        build: bool,

        /// Pass --no-cache to docker build.
        #[arg(long)]
        no_cache: bool,

        /// Run the agent in non-interactive (print) mode instead of interactive mode.
        #[arg(long)]
        non_interactive: bool,

        /// Mount the host Docker daemon socket into the agent container.
        #[arg(long)]
        allow_docker: bool,
    },

    /// Launch the dev container to implement a work item.
    Implement {
        /// Work item number (e.g. 0001).
        work_item: String,

        /// Run the agent in non-interactive (print) mode instead of interactive mode.
        #[arg(long)]
        non_interactive: bool,

        /// Run the agent in plan mode (read-only, no file modifications).
        #[arg(long)]
        plan: bool,

        /// Mount the host Docker daemon socket into the agent container.
        #[arg(long)]
        allow_docker: bool,
    },

    /// Start a freeform chat session with the configured agent in a container.
    Chat {
        /// Run the agent in non-interactive (print) mode instead of interactive mode.
        #[arg(long)]
        non_interactive: bool,

        /// Run the agent in plan mode (read-only, no file modifications).
        #[arg(long)]
        plan: bool,

        /// Mount the host Docker daemon socket into the agent container.
        #[arg(long)]
        allow_docker: bool,
    },

    /// Create a new work item from the template.
    New,
}

#[derive(Clone, Debug, ValueEnum)]
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
        let cli = parse(&["aspec", "ready", "--refresh", "--build", "--no-cache", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Ready { refresh, build, no_cache, non_interactive, .. } => {
                assert!(refresh);
                assert!(build);
                assert!(no_cache);
                assert!(non_interactive);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_defaults_no_refresh_no_non_interactive() {
        let cli = parse(&["aspec", "ready"]);
        match cli.command.unwrap() {
            Command::Ready { refresh, build, no_cache, non_interactive, allow_docker } => {
                assert!(!refresh);
                assert!(!build);
                assert!(!no_cache);
                assert!(!non_interactive);
                assert!(!allow_docker);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_build_flag() {
        let cli = parse(&["aspec", "ready", "--build"]);
        match cli.command.unwrap() {
            Command::Ready { build, .. } => assert!(build),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_no_cache_flag() {
        let cli = parse(&["aspec", "ready", "--no-cache"]);
        match cli.command.unwrap() {
            Command::Ready { no_cache, .. } => assert!(no_cache),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_build_and_no_cache_flags() {
        let cli = parse(&["aspec", "ready", "--build", "--no-cache"]);
        match cli.command.unwrap() {
            Command::Ready { build, no_cache, .. } => {
                assert!(build);
                assert!(no_cache);
            }
            _ => panic!("expected ready"),
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

    #[test]
    fn chat_subcommand_parsed() {
        let cli = parse(&["aspec", "chat"]);
        assert!(matches!(cli.command.unwrap(), Command::Chat { .. }));
    }

    #[test]
    fn chat_defaults_interactive() {
        let cli = parse(&["aspec", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { non_interactive, .. } => assert!(!non_interactive),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_non_interactive_flag() {
        let cli = parse(&["aspec", "chat", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Chat { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_plan_flag() {
        let cli = parse(&["aspec", "chat", "--plan"]);
        match cli.command.unwrap() {
            Command::Chat { plan, non_interactive, .. } => {
                assert!(plan);
                assert!(!non_interactive);
            }
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_defaults_no_plan() {
        let cli = parse(&["aspec", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { plan, .. } => assert!(!plan),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_plan_and_non_interactive() {
        let cli = parse(&["aspec", "chat", "--plan", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Chat { plan, non_interactive, .. } => {
                assert!(plan);
                assert!(non_interactive);
            }
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn implement_plan_flag() {
        let cli = parse(&["aspec", "implement", "0001", "--plan"]);
        match cli.command.unwrap() {
            Command::Implement { plan, work_item, non_interactive, .. } => {
                assert!(plan);
                assert_eq!(work_item, "0001");
                assert!(!non_interactive);
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_defaults_no_plan() {
        let cli = parse(&["aspec", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { plan, .. } => assert!(!plan),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_plan_and_non_interactive() {
        let cli = parse(&["aspec", "implement", "0001", "--plan", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Implement { plan, non_interactive, .. } => {
                assert!(plan);
                assert!(non_interactive);
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn root_build_flag() {
        let cli = parse(&["aspec", "--build"]);
        assert!(cli.build);
        assert!(!cli.no_cache);
        assert!(!cli.refresh);
        assert!(cli.command.is_none());
    }

    #[test]
    fn root_no_cache_flag() {
        let cli = parse(&["aspec", "--no-cache"]);
        assert!(cli.no_cache);
        assert!(!cli.build);
        assert!(!cli.refresh);
    }

    #[test]
    fn root_refresh_flag() {
        let cli = parse(&["aspec", "--refresh"]);
        assert!(cli.refresh);
        assert!(!cli.build);
        assert!(!cli.no_cache);
    }

    #[test]
    fn root_all_flags() {
        let cli = parse(&["aspec", "--build", "--no-cache", "--refresh"]);
        assert!(cli.build);
        assert!(cli.no_cache);
        assert!(cli.refresh);
        assert!(cli.command.is_none());
    }

    #[test]
    fn root_flags_default_false() {
        let cli = parse(&["aspec"]);
        assert!(!cli.build);
        assert!(!cli.no_cache);
        assert!(!cli.refresh);
    }

    // --- --allow-docker flag tests ---

    #[test]
    fn implement_allow_docker_flag() {
        let cli = parse(&["aspec", "implement", "0001", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Implement { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_defaults_no_allow_docker() {
        let cli = parse(&["aspec", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_allow_docker_with_plan() {
        let cli = parse(&["aspec", "implement", "0001", "--allow-docker", "--plan"]);
        match cli.command.unwrap() {
            Command::Implement { allow_docker, plan, .. } => {
                assert!(allow_docker);
                assert!(plan);
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn chat_allow_docker_flag() {
        let cli = parse(&["aspec", "chat", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Chat { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_defaults_no_allow_docker() {
        let cli = parse(&["aspec", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_allow_docker_with_plan() {
        let cli = parse(&["aspec", "chat", "--allow-docker", "--plan"]);
        match cli.command.unwrap() {
            Command::Chat { allow_docker, plan, .. } => {
                assert!(allow_docker);
                assert!(plan);
            }
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn ready_allow_docker_flag() {
        let cli = parse(&["aspec", "ready", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_defaults_no_allow_docker() {
        let cli = parse(&["aspec", "ready"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_allow_docker_with_refresh() {
        let cli = parse(&["aspec", "ready", "--allow-docker", "--refresh"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, refresh, .. } => {
                assert!(allow_docker);
                assert!(refresh);
            }
            _ => panic!("expected ready"),
        }
    }
}
