# DiWop Begleitapp Backend

Welcome to the `begleitapp-backend` repository.

## APIs

This service exposes two APIs:

### [Translations API](proto/translations.proto)

It provides translations into simple language.

### [Management API](proto/management.proto)

For now it just provides listing users (always an empty list).

## Tech Structure

This repository contains a Rust-based backend service. It acts as an integration and API layer, powered by the following core technologies:

- **Rust**: The core programming language used for high-performance and safe execution.
- **gRPC APIs**: APIs are defined in proto files and exposed via gRPC.
- **JSON APIs**: The gRPC APIs are also exposed as JSON REST APIs for easier consumption by humans.
- **Swagger UI**: The JSON APIs are documented using Swagger UI at `/open-api`.
- **Docker**: Containerization for easy deployment and management.

## Running modes

`cargo run --release` is intended for production within the docker container. It uses structured JSON logging for STACKIT.

`cargo run` is intended for local development and uses human-readable colored logging.

## Onboarding and Setup Workflows

This project utilizes specific agentic workflows (located in `.agent/workflows/`) to assist with onboarding, installation, and GitHub repository configuration. These workflows are designed to be run by AI coding assistants (like Google Antigravity) to guide new developers.

- [`onboarding.md`](.agent/workflows/onboarding.md): Guides developers through installing necessary system requirements (`git`, `cargo`, `cargo nextest`, `protoc`) and setting up local MCP (Model Context Protocol) servers.
- [`initial-setup.md`](.agent/workflows/initial-setup.md): A guide for the initial project setup, including GitHub repository configuration and baseline application installation.
- [`configure-github-repo.md`](.agent/workflows/configure-github-repo.md): Sets up repository rules, merge strategies, issues, and wikis.

### Running the Onboarding Workflow in Google Antigravity

If you are a new developer setting up this repository, it is highly suggested that you begin with the **Onboarding** workflow to verify your system dependencies and configure the environment.

To run the onboarding workflow using **Google Antigravity**, simply type the following slash command in the chat:

```text
/onboarding
```

The assistant will guide you step-by-step through installing any missing dependencies and ensuring your system is correctly prepared for local development.
