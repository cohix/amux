# Work Item: Task

Title: new tool vision pivot
Issue: issuelink

## Summary:
- this tool is being renamed from aspec-cli to amux


## Implementation Details:
- replace `aspec-cli` and the `aspec` top-level command naming within the code to be `amux` instead
- the new project purpose is to be "a containerized code and claw agent manager"
- rewrite docs and readme along with code to refer to this tool - not the concept of aspec that it manages - to be amux


## Edge Case Considerations:
- the folder downloaded during `init` is still called `aspec` ( still downloaded from the aspec repo )
- the files/folders that make up a project's `aspec` are still being called `aspec`, but this tool that is used to manage the `aspec` and the agents that use it is being renamed.

## Test Considerations:
- just update the tests for the new name

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
