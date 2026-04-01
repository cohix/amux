# Work Item: Enhancement

Title: Template Dockerfile Updates
Issue: issuelink

## Summary:
- update the templates for Dockerfile.dev to use best practices

## User Stories

### User Story 1:
As a: user

I want to:
recieve a Dockerfile.dev that uses best practices for small images, package-manager installed packages, etc.

So I can:
have an efficient and secure development environment


## Implementation Details:
- update all templates for Dockerfile.dev (NOT the Dockerfile.dev for this project, templates only) to use the `debian-slim` base image
- wherever possible, use apt-get or direct install for code agents rather than npm or any other method
- there is a bug when the `ready` command is run: if an existing Dockerfile.dev exists, it is overwritten with the template. Fix that bug. A template should only be used if none exists.


## Edge Case Considerations:
- do not change base images for convenience of installing specfic tools.
- prefer a more complex but more secure install method over convenience.

## Test Considerations:
- build each of the template Dockerfiles (for claude, codex, opencode) to ensure they build successfuly in their default/template states.

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.