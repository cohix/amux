# Work Item: Feature

Title: claw init improvements
Issue: issuelink

## Summary:
- improved audit prompt specifically for nanoclaw setup

## User Stories

### User Story 1:
As a: user

I want to:
have the claws init audit container send a more comprehensive setup prompt

So I can:
get through nanoclaw setup more quickly


## Implementation Details:
- when running `claws init`, an agent audit container is run to configure Dockerfile.dev. The prompt that is sent to this container must be changed to also configure more of the nanoclaw codebase beyond Dockerfile.dev
- add a new prompt that is specific for the nanoclaw audit agent container. this prompt will be seperate from the audit prompt that is normally sent as part of `init` or `ready --refresh`. Modify the first run experience in `claws init` to use this new prompt, do not edit other uses of the audit container, this change is only for `claws init`.
- the new prompt is shown below. use this new prompt only for the audit container used in `claws init`.

Prompt:

```
First, scan this project and determine every tool needed to build, run, and test it per the configuration workflows, and code languages present. Modify Dockerfile.dev to ensure that all of those tools, at the newest compatible version, get installed when the Dockerfile is built. Pin to specific versions wherever possible. Use the OS package manager or direct-from-website installers whenever possible, prefer those methods over npm. Do not change the Claude install method. Think hard to ensure the Dockerfile.dev will build cleanly, but do not attempt to build it, since you do not have access to Docker. Ensure that all relevant tools are in $PATH and can be executed by the container entrypoint command. Only modify Dockerfile.dev; do not modify any other files. Do not add any new files.

Next, evaluate the nanoclaw source code to determine how containers communicate with each other over the network. Modify the codebase so that all network traffic gets configured to be container-to-container rather than container-to-host. The `host.docker.internal` route should never be used, since nanoclaw, onecli, and all workers will be running within containers, and so communication with the host machine will never be needed. The nanoclaw controller should communicate with onecli over a container-to-container connection. When worker containers are launched, they should be configured to communicate with onecli and the nanoclaw controller over a container-to-container connection. Use the Docker CLI and container ports to determine how to perform all network communication. Modify the codebase comprehensively, looking for any place where network traffic is configured, including addresses and ports for the controller onecli, and workers. Think hard to ensure no container-to-host communication will happen after your changes are complete.
```

## Edge Case Considerations:
- only add this new prompt and use it in the `claws init` agent audit container, no other changes. The agent audit container should be passed this prompt when it starts, and should be run FOREGROUND, ATTACHED, PROMPTED.

## Test Considerations:
- considerations

## Codebase Integration:
- follow established conventions, best practices, testing, and architecture patterns from the project's aspec.
