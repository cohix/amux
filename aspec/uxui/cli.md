# CLI Design

Binary name: aspec
Install path: /usr/local/bin/
Storage location: $HOME/.aspec/

## Design principles:

### Command structure
Top level command groups:
- aspec (no arguments): launches "interactive mode" repl using a Ratatui TUI
- aspec init: initializes the current Git repo (detect the Git root) to be used with aspec. --agent=[claude|codex|opencode] flag configures which agentic tool will be installed in the Dockerfile.dev container.
- aspec ready: ensures the local Docker daemon is running and accessible, checks that Dockerfile.dev is present, builds it into a local Docker image, and reports status back to user.
- aspec implement <work item number>: launches the Doocker image (built from Dockerfile.dev) with the user's preferred code agent to implement the indicated work item from the project's aspec folder.

### Flag structure
Flag guidance:
- guidance

### Inputs and outputs
I/O Guidance:
- stdin
- stdout
- any Docker containers launched should plumb the developer machine's stdin, stdout, stderr to the running container so that the user can interact with the conatiner within the aspec interactive TUI.

### Configuration
Global config:
- store configuration for a specific Git repo within a JSON file: GITROOT/aspec/.aspec-cli.json
- store global config within `$HOME/.aspec/config.json