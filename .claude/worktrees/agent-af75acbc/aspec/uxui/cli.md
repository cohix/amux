# CLI Design

Binary name: amux
Install path: /usr/local/bin/
Storage location: $HOME/.amux/

## Design principles:

### Command structure
Top level command groups:
- amux (no arguments): launches "interactive mode" repl using a Ratatui TUI
- amux init: initializes the current Git repo (detect the Git root) to be used with amux. --agent=[claude|codex|opencode] flag configures which agentic tool will be installed in the Dockerfile.dev container.
- amux ready: ensures the local Docker daemon is running and accessible, checks that Dockerfile.dev is present, builds it into a local Docker image, and reports status back to user.
- amux implement <work item number>: launches the Docker image (built from Dockerfile.dev) with the user's preferred code agent to implement the indicated work item from the project's aspec folder.
- amux chat: launches the Docker image (built from Dockerfile.dev) with the user's preferred code agent for a freeform interactive chat session with no pre-configured prompt.

### Flag structure
Flag guidance:
- guidance

### Inputs and outputs
I/O Guidance:
- stdin
- stdout
- any Docker containers launched should plumb the developer machine's stdin, stdout, stderr to the running container so that the user can interact with the conatiner within the amux interactive TUI.

### Configuration
Global config:
- store configuration for a specific Git repo within a JSON file: GITROOT/aspec/.amux.json
- store global config within `$HOME/.amux/config.json