# Work Item: Task

Title: prepare for users
Issue: issuelink

## Summary:
- prepare the repo and documentation for users

## User Stories

### User Story 1:
As a: aspec-cli repo owner

I want to:
improve the readme, documentation, and Github pipelines

So I can:
provide a good first experience for new users

## Implementation Details:
- first, implement a GitHub pipeline which will run all of the aspec tests for every commit on every branch
- next, implement a GitHub pipeline which builds windows, linux, and macos release binaries for any `vx.y.z` tag that gets pushed to the main branch
	- the pipeline should upload binaries to Github releases for the correct tag upon successful build
- next, add a `make release vx.y.z` target which:
	- switch to main branch, ensure latest is pulled and no conflicts or git issues exist
	- creates a new file in `docs/releases/vx.y.x.md` to allow writing release notes
	- launches `aspec chat` so the user can prompt their agent to write release notes
	- once the chat completes, run all tests locally
	- if tests pass, commit the new release notes to a new commit on main
	- tag that new commit with the version number provided
	- push the commit and tag to the main branch
	- use the gh CLI to upload the release notes to the proper tag (from the markdown file)
- next, add a "getting started guide" markdown file in the docs folder which covers the basics of installation and a simple workflow involving the init, ready, chat, and implement subcommands. Link to the relevant parts of the usage file for further reference.
- finally, update the project readme to be catchy, informative about the benefits of spec-driven development and the security benefits of aspec, link to the main aspec repo, and link to the getting started guide and documentation. Also include a Github badge showing the current state of the tests in the pipeline.

## Edge Case Considerations:
- considerations

## Test Considerations:
- considerations

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
- don't touch any rust code for this one
