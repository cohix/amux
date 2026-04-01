use clap::{Parser, Subcommand, ValueEnum};

/// A containerized code and claw agent manager.
#[derive(Parser)]
#[command(name = "amux", version)]
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
    /// Initialize the current Git repo for use with amux.
    Init {
        /// Code agent to install in the Dockerfile.dev container.
        #[arg(long, value_enum, default_value = "claude")]
        agent: Agent,
        /// Download aspec templates to the current project.
        #[arg(long)]
        aspec: bool,
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

        /// Path to a workflow Markdown file. If omitted, the work item is implemented
        /// in a single agent run with the current prompt, unchanged.
        #[arg(long)]
        workflow: Option<std::path::PathBuf>,
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

    /// Manage work item specs (create, interview, amend).
    Specs {
        #[command(subcommand)]
        action: SpecsAction,
    },

    /// Manage persistent background agent containers (claws agents).
    Claws {
        #[command(subcommand)]
        action: ClawsAction,
    },

    /// Show the status of all running code-agent and nanoclaw containers.
    Status {
        /// Continuously refresh the output every 3 seconds.
        #[arg(long)]
        watch: bool,
    },
}

/// Subcommands for `amux specs`.
#[derive(Subcommand)]
pub enum SpecsAction {
    /// Create a new work item from the template.
    New {
        /// Use interview mode: have the agent complete the work item based on a summary you provide.
        #[arg(long)]
        interview: bool,
    },
    /// Review and amend a completed work item to match the final implementation.
    Amend {
        /// Work item number (e.g. 0025).
        work_item: String,
        /// Run the agent in non-interactive (print) mode.
        #[arg(long)]
        non_interactive: bool,
        /// Mount the host Docker daemon socket into the agent container.
        #[arg(long)]
        allow_docker: bool,
    },
}

/// Subcommands for `amux claws`.
#[derive(Subcommand)]
pub enum ClawsAction {
    /// First-time setup: fork/clone nanoclaw, build the image, and launch the container.
    Init,
    /// Check whether the nanoclaw container is running and show status.
    Ready,
    /// Attach to the running nanoclaw container for a freeform chat session.
    Chat,
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
        let cli = parse(&["amux"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn init_default_agent_is_claude() {
        let cli = parse(&["amux", "init"]);
        match cli.command.unwrap() {
            Command::Init { agent, .. } => assert_eq!(agent.as_str(), "claude"),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_explicit_agent() {
        let cli = parse(&["amux", "init", "--agent", "codex"]);
        match cli.command.unwrap() {
            Command::Init { agent, .. } => assert_eq!(agent.as_str(), "codex"),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_aspec_flag_false_by_default() {
        let cli = parse(&["amux", "init"]);
        match cli.command.unwrap() {
            Command::Init { aspec, .. } => assert!(!aspec),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_aspec_flag_set() {
        let cli = parse(&["amux", "init", "--aspec"]);
        match cli.command.unwrap() {
            Command::Init { aspec, .. } => assert!(aspec),
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn init_aspec_with_agent() {
        let cli = parse(&["amux", "init", "--aspec", "--agent", "codex"]);
        match cli.command.unwrap() {
            Command::Init { agent, aspec } => {
                assert_eq!(agent.as_str(), "codex");
                assert!(aspec);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn implement_parses_work_item_number() {
        let cli = parse(&["amux", "implement", "42"]);
        match cli.command.unwrap() {
            Command::Implement { work_item, .. } => assert_eq!(work_item, "42"),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_workflow_flag_some() {
        let cli = parse(&["amux", "implement", "0001", "--workflow", "wf.md"]);
        match cli.command.unwrap() {
            Command::Implement { workflow, .. } => {
                assert_eq!(workflow, Some(std::path::PathBuf::from("wf.md")));
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_workflow_flag_none_by_default() {
        let cli = parse(&["amux", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { workflow, .. } => assert!(workflow.is_none()),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_workflow_with_other_flags() {
        let cli = parse(&["amux", "implement", "0001", "--workflow", "my-wf.md", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Implement { workflow, non_interactive, .. } => {
                assert!(workflow.is_some());
                assert!(non_interactive);
            }
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_parses_four_digit_work_item() {
        let cli = parse(&["amux", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { work_item, .. } => assert_eq!(work_item, "0001"),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn ready_subcommand_parsed() {
        let cli = parse(&["amux", "ready"]);
        assert!(matches!(cli.command.unwrap(), Command::Ready { .. }));
    }

    #[test]
    fn ready_refresh_flag() {
        let cli = parse(&["amux", "ready", "--refresh"]);
        match cli.command.unwrap() {
            Command::Ready { refresh, .. } => assert!(refresh),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_non_interactive_flag() {
        let cli = parse(&["amux", "ready", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Ready { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_all_flags() {
        let cli = parse(&["amux", "ready", "--refresh", "--build", "--no-cache", "--non-interactive"]);
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
        let cli = parse(&["amux", "ready"]);
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
        let cli = parse(&["amux", "ready", "--build"]);
        match cli.command.unwrap() {
            Command::Ready { build, .. } => assert!(build),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_no_cache_flag() {
        let cli = parse(&["amux", "ready", "--no-cache"]);
        match cli.command.unwrap() {
            Command::Ready { no_cache, .. } => assert!(no_cache),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_build_and_no_cache_flags() {
        let cli = parse(&["amux", "ready", "--build", "--no-cache"]);
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
        let cli = parse(&["amux", "implement", "0001", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Implement { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_defaults_interactive() {
        let cli = parse(&["amux", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { non_interactive, .. } => assert!(!non_interactive),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn chat_subcommand_parsed() {
        let cli = parse(&["amux", "chat"]);
        assert!(matches!(cli.command.unwrap(), Command::Chat { .. }));
    }

    #[test]
    fn chat_defaults_interactive() {
        let cli = parse(&["amux", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { non_interactive, .. } => assert!(!non_interactive),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_non_interactive_flag() {
        let cli = parse(&["amux", "chat", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Chat { non_interactive, .. } => assert!(non_interactive),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_plan_flag() {
        let cli = parse(&["amux", "chat", "--plan"]);
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
        let cli = parse(&["amux", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { plan, .. } => assert!(!plan),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_plan_and_non_interactive() {
        let cli = parse(&["amux", "chat", "--plan", "--non-interactive"]);
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
        let cli = parse(&["amux", "implement", "0001", "--plan"]);
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
        let cli = parse(&["amux", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { plan, .. } => assert!(!plan),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_plan_and_non_interactive() {
        let cli = parse(&["amux", "implement", "0001", "--plan", "--non-interactive"]);
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
        let cli = parse(&["amux", "--build"]);
        assert!(cli.build);
        assert!(!cli.no_cache);
        assert!(!cli.refresh);
        assert!(cli.command.is_none());
    }

    #[test]
    fn root_no_cache_flag() {
        let cli = parse(&["amux", "--no-cache"]);
        assert!(cli.no_cache);
        assert!(!cli.build);
        assert!(!cli.refresh);
    }

    #[test]
    fn root_refresh_flag() {
        let cli = parse(&["amux", "--refresh"]);
        assert!(cli.refresh);
        assert!(!cli.build);
        assert!(!cli.no_cache);
    }

    #[test]
    fn root_all_flags() {
        let cli = parse(&["amux", "--build", "--no-cache", "--refresh"]);
        assert!(cli.build);
        assert!(cli.no_cache);
        assert!(cli.refresh);
        assert!(cli.command.is_none());
    }

    #[test]
    fn root_flags_default_false() {
        let cli = parse(&["amux"]);
        assert!(!cli.build);
        assert!(!cli.no_cache);
        assert!(!cli.refresh);
    }

    #[test]
    fn status_subcommand_parsed() {
        let cli = parse(&["amux", "status"]);
        assert!(matches!(cli.command.unwrap(), Command::Status { .. }));
    }

    #[test]
    fn status_defaults_no_watch() {
        let cli = parse(&["amux", "status"]);
        match cli.command.unwrap() {
            Command::Status { watch } => assert!(!watch),
            _ => panic!("expected status"),
        }
    }

    #[test]
    fn status_watch_flag() {
        let cli = parse(&["amux", "status", "--watch"]);
        match cli.command.unwrap() {
            Command::Status { watch } => assert!(watch),
            _ => panic!("expected status"),
        }
    }

    // --- --allow-docker flag tests ---

    #[test]
    fn implement_allow_docker_flag() {
        let cli = parse(&["amux", "implement", "0001", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Implement { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_defaults_no_allow_docker() {
        let cli = parse(&["amux", "implement", "0001"]);
        match cli.command.unwrap() {
            Command::Implement { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected implement"),
        }
    }

    #[test]
    fn implement_allow_docker_with_plan() {
        let cli = parse(&["amux", "implement", "0001", "--allow-docker", "--plan"]);
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
        let cli = parse(&["amux", "chat", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Chat { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_defaults_no_allow_docker() {
        let cli = parse(&["amux", "chat"]);
        match cli.command.unwrap() {
            Command::Chat { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected chat"),
        }
    }

    #[test]
    fn chat_allow_docker_with_plan() {
        let cli = parse(&["amux", "chat", "--allow-docker", "--plan"]);
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
        let cli = parse(&["amux", "ready", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, .. } => assert!(allow_docker),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_defaults_no_allow_docker() {
        let cli = parse(&["amux", "ready"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, .. } => assert!(!allow_docker),
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn ready_allow_docker_with_refresh() {
        let cli = parse(&["amux", "ready", "--allow-docker", "--refresh"]);
        match cli.command.unwrap() {
            Command::Ready { allow_docker, refresh, .. } => {
                assert!(allow_docker);
                assert!(refresh);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn claws_ready_parsed() {
        let cli = parse(&["amux", "claws", "ready"]);
        assert!(matches!(
            cli.command.unwrap(),
            Command::Claws { action: ClawsAction::Ready }
        ));
    }

    #[test]
    fn claws_ready_is_ready_action() {
        let cli = parse(&["amux", "claws", "ready"]);
        match cli.command.unwrap() {
            Command::Claws { action } => assert!(matches!(action, ClawsAction::Ready)),
            _ => panic!("expected claws"),
        }
    }

    #[test]
    fn claws_init_parsed() {
        let cli = parse(&["amux", "claws", "init"]);
        assert!(matches!(
            cli.command.unwrap(),
            Command::Claws { action: ClawsAction::Init }
        ));
    }

    #[test]
    fn claws_init_is_init_action() {
        let cli = parse(&["amux", "claws", "init"]);
        match cli.command.unwrap() {
            Command::Claws { action } => assert!(matches!(action, ClawsAction::Init)),
            _ => panic!("expected claws"),
        }
    }

    #[test]
    fn claws_chat_parsed() {
        let cli = parse(&["amux", "claws", "chat"]);
        assert!(matches!(
            cli.command.unwrap(),
            Command::Claws { action: ClawsAction::Chat }
        ));
    }

    #[test]
    fn claws_chat_is_chat_action() {
        let cli = parse(&["amux", "claws", "chat"]);
        match cli.command.unwrap() {
            Command::Claws { action } => assert!(matches!(action, ClawsAction::Chat)),
            _ => panic!("expected claws"),
        }
    }

    #[test]
    fn specs_new_parsed() {
        let cli = parse(&["amux", "specs", "new"]);
        match cli.command.unwrap() {
            Command::Specs { action: SpecsAction::New { interview } } => assert!(!interview),
            _ => panic!("expected specs new"),
        }
    }

    #[test]
    fn specs_new_interview_flag() {
        let cli = parse(&["amux", "specs", "new", "--interview"]);
        match cli.command.unwrap() {
            Command::Specs { action: SpecsAction::New { interview } } => assert!(interview),
            _ => panic!("expected specs new --interview"),
        }
    }

    #[test]
    fn specs_amend_parsed() {
        let cli = parse(&["amux", "specs", "amend", "0025"]);
        match cli.command.unwrap() {
            Command::Specs { action: SpecsAction::Amend { work_item, non_interactive, allow_docker } } => {
                assert_eq!(work_item, "0025");
                assert!(!non_interactive);
                assert!(!allow_docker);
            }
            _ => panic!("expected specs amend"),
        }
    }

    #[test]
    fn specs_amend_non_interactive_flag() {
        let cli = parse(&["amux", "specs", "amend", "0025", "--non-interactive"]);
        match cli.command.unwrap() {
            Command::Specs { action: SpecsAction::Amend { non_interactive, .. } } => {
                assert!(non_interactive);
            }
            _ => panic!("expected specs amend --non-interactive"),
        }
    }

    #[test]
    fn specs_amend_allow_docker_flag() {
        let cli = parse(&["amux", "specs", "amend", "0025", "--allow-docker"]);
        match cli.command.unwrap() {
            Command::Specs { action: SpecsAction::Amend { allow_docker, .. } } => {
                assert!(allow_docker);
            }
            _ => panic!("expected specs amend --allow-docker"),
        }
    }

    #[test]
    fn claws_actions_are_distinct() {
        let init = parse(&["amux", "claws", "init"]);
        let ready = parse(&["amux", "claws", "ready"]);
        let chat = parse(&["amux", "claws", "chat"]);
        assert!(matches!(
            init.command.unwrap(),
            Command::Claws { action: ClawsAction::Init }
        ));
        assert!(matches!(
            ready.command.unwrap(),
            Command::Claws { action: ClawsAction::Ready }
        ));
        assert!(matches!(
            chat.command.unwrap(),
            Command::Claws { action: ClawsAction::Chat }
        ));
    }
}
