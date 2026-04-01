# Work Item: Feature

Title: Download Templates and aspec from GH
Issue: issuelink

## Summary:
- download templates and aspec folder from GH when needed

## User Stories

### User Story 1:
As a: user

I want to:
get the latest `aspec` folder and Dockerfile templates from GitHub instead of local copies

So I can:
stay on the latest versions without updating the CLI


## Implementation Details:
- for the `init` subcommand, download the `aspec` folder from github.com/cohix/aspec when initializing the project. The repo may need to be cloned or downloaded into a temporary folder and then copied rather than downloaded directly since there are many folders and files.
	- only include the `aspec` folder within the `aspec` repo when initializing, not the entire repo.
- for the `init` and `ready` subcommands, download Dockerfile.dev templates for the configued agent from github.com/cohix/aspec-cli/... rather than using local copies, as those will not be found on machines that do not have `aspec` or `aspec-cli` repos cloned
- when downloading anything, write a log message indicating what is being download from where, and show the result including where it was downloaded to and the file size


## Edge Case Considerations:
- audit the remainder of the codebase and find any places where assumptions were made about files being available on the local machine and write them as notes in the bottom of this work item file for follow-up if needed.
- ensure parity between CLI and TUI.

## Test Considerations:
- add unit and integration tests for file/folder downloading

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.

## Audit Notes (local file assumptions):
- `commands/new.rs:find_template()` — previously assumed `aspec/work-items/0000-template.md` existed locally and showed an error with a manual download link if missing. **Fixed**: now auto-downloads the aspec folder from GitHub when the template is not found.
- `commands/init.rs:dockerfile_for_agent()` — previously used `include_str!()` to embed templates at compile time. **Fixed**: now downloads from GitHub with embedded templates as fallback.
- `commands/ready.rs:run_pre_audit()` — called `write_dockerfile()` which used embedded templates. **Fixed**: now uses the async download-with-fallback version.
- `templates/Dockerfile.*` — still present in the repo as embedded fallbacks. No action needed; they serve as offline fallback when GitHub is unreachable.
- No other local file assumptions were found outside of git root detection (`find_git_root`) and config loading, which are expected to be local.
