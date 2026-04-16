---
name: Server Development
description: Develop and test features in the server
---

# Change Server

## Concepts

The project contains a webserver written in Rust.

### Testing

If you introduce a new feature or make a change it has to be reflected in tests.
If there are existing unit or integration tests, extend or update them.
If not, evaluate if unit and/or integration tests are appropriate to test the change.

## MCP Tools

The `context7` MCP server is available for Rust crate documentation.

- Use `resolve-library-id` and `query-docs` to find documentation and examples for Rust crates.

### Final Checks (CRITICAL)

At the end of development run `cargo format`, `cargo clippy` and `cargo nextest run`.
