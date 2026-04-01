# Local Development

Development: local
Build tools: make, cargo, docker

## Workflows:

Developer Loop:
- running `make all` should build the aspec CLI binary using the local Rust/Cargo toolchain
- running `make install` should run `make all` and then install the aspec CLI to /usr/local/bin/


Local testing:
- running `make test` should run all tests in the project

Version control:
- Git is used for this project

Documentation:
- After every work item is implemented, documentation should be written within the docs/ folder. Do not create one document per work item, but instead author a comprehensive set of documentation that explains to the user how to use the aspec tool in its entirety. Each work item should trigger an inspection of the entire docs/ folder to update and/or add relevant usage information.