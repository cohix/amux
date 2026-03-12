# aspec-cli

## Source of Truth

All decisions — architecture, behavior, security, UX, and implementation — are governed by the spec files in `aspec/`. Before writing or modifying code, consult the relevant spec document(s).

| Concern | Spec file |
|---|---|
| Project purpose, language, personas | `aspec/foundation.md` |
| Architecture and design principles | `aspec/architecture/design.md` |
| Security constraints | `aspec/architecture/security.md` |
| CLI commands, flags, config | `aspec/uxui/cli.md` |
| Local dev, build, test workflows | `aspec/devops/localdev.md` |
| CI/CD | `aspec/devops/cicd.md` |
| Operations | `aspec/devops/operations.md` |
| Subagents | `aspec/devops/subagents.md` |
| Work item template | `aspec/work-items/0000-template.md` |

## Tech Stack

- **Language**: Rust (idiomatic, async)
- **TUI framework**: Ratatui
- **Build tools**: `make`, `cargo`, `docker`
- **Output**: single statically-linked binary (`aspec`)

## Build & Test Commands

```
make all      # build the aspec binary
make install  # build + install to /usr/local/bin/
make test     # run all tests
```

## Architecture Constraints

- Single statically-linked binary for macOS, Linux, Windows
- Two execution modes: interactive (TUI via Ratatui) and command (single invocation, stdout/stderr output)
- Interacts only with the active Git repo and the local Docker daemon
- Prefer simplicity over cleverness — readable by an intermediate Rust developer
- Small, modular crates

## Security Constraints (non-negotiable)

- **Never execute any agent/code assistant directly on the host machine**
- All agentic operations run inside Docker containers (built from `Dockerfile.dev`)
- Only mount the current directory or Git repo root to containers — never parent directories
- If a parent directory is the Git root, prompt the user to confirm the mount scope

## Testing

- Every module/function: unit tests validating inputs and outputs
- Cross-component flows: integration tests
- Full CLI behavior: end-to-end tests

## Configuration

- Per-repo config: `GITROOT/aspec/.aspec-cli.json`
- Global config: `$HOME/.aspec/config.json`
- Binary install path: `/usr/local/bin/`
- Data/storage: `$HOME/.aspec/`

## Work Items

Work items live in `aspec/work-items/`. Use `aspec/work-items/0000-template.md` as the template for new work items.

After implementing a work item:
1. Review and update `docs/` to reflect the current state of the tool (comprehensive docs, not per-work-item)
2. Follow the patterns, conventions, and architecture established in the `aspec/` spec
