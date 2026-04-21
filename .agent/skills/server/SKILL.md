---
name: Server Development
description: Develop and test features in the server
---

# Change Server

The project contains a webserver written in Rust.

Always check for compilation errors with `cargo check` and run `cargo nextest run` before notifying the user about the completion of a task.

## Testing

**CRITICAL**: You **MUST** write unit or integration tests for any newly added feature or component. Do not finish a task without providing corresponding test coverage.
If there are existing unit, integration, or end-to-end tests, extend or update them.

**CRITICAL**: You **MUST** write the test for a new pure function at the same time.

## Comments

Add descriptive comments to every function, struct, or construct that has more than 3 lines of code.
**CRITICAL:** If you change a function, struct, or construct that has a comment, you **MUST** update the comment as well. Check the comment for correctness and completeness and update it if necessary.

## MCP Tools

The `context7` MCP server is available for Rust crate documentation.

- Use `resolve-library-id` and `query-docs` to find documentation and examples for Rust crates.

### Final Checks (CRITICAL)

At the end of development run `cargo format`, `cargo clippy` and `cargo nextest run`.
