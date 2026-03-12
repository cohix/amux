# aspec CLI

A CLI tool for managing predictable and secure agentic coding environments.
Every agent action runs inside a Docker container — never directly on your machine.

## Quick Start

```sh
make install        # build and install to /usr/local/bin/
aspec init          # initialise your repo
aspec ready         # verify Docker + build dev container
aspec implement 1   # run work item 0001 with your chosen agent
aspec               # open the interactive TUI
```

## Documentation

- [Usage Guide](docs/usage.md) — commands, TUI reference, agent auth
- [Architecture](docs/architecture.md) — code structure, state machine, PTY design
