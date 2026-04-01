# Security

## Guidance:

- Never directly execute any code assistant on the developer host machine. Every single code agent action should be executed by running a Docker image that has the agent tool installed and passing an entrypoing command to direct the agentic tool.
- Never mount any directory to any Docker container other than the current directory. If any parent directories are a Git repo root, the aspec CLI will prompt the user if the mounted directory should be limited to the current CWD or can be expanded to the Git repo root. Follow this instruction for every single container launched.