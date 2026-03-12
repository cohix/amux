# Project Foundation

Name: aspec-cli
Type: CLI
Purpose: A CLI tool specifically designed to manage predictable and secure agentic coding environments.

# Technical Foundation

## Languages and Frameworks

### CLI
Language: Rust
Frameworks: Ratatui
Guidance:
- The `aspec` CLI should compile to a single, statically linked binary for macOS, Linux, and Windows.
- Every function of the CLI should be accessible either in "interactive" mode (i.e. running `aspec` with no arguments launches a TUI to interact with its features) or "command" mode, where `aspec` is run with one or more arguments, executes a single function, and then exits, printing its output to stdout and stderr.
- Idiomatic, async Rust code
- Small, easily understood modules and crates
- Prefer simplicity (understandable by an intermediate Rust programmer) over complex code that is concise.

# Best Practices
- Organize code in small, simple, modular components
- Each component should contain unit tests that validate its behaviour in terms of inputs and outputs
- The overall codebase should contain integration tests that validate the interation between components that are used together

# Personas

### Persona 1:
Name: user
Purpose: user of the `aspec` CLI tool in their macOS, linux, or Windows terminal.
Use-cases:
- executing `aspec` interactive mode for ongoing sessions
- executing `aspec <>` command mode for single-use commands
RBAC:
- allowed: all
- disallowed: none