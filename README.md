# DiWop Begleit-App Backend

Welcome to the `begleit-app-backend` repository.

See [docs/implementation-details.md](docs/implementation-details.md) for technical information.

## APIs

This service exposes two APIs:

### [Translations API](proto/translations.proto)

It provides translations into simple language.

### [Management API](proto/management.proto)

For now it just provides listing users (always an empty list).

## Getting Started

Use the agentic workflow [`/onboarding`](.agent/workflows/onboarding.md) to get started.

Start the server with `cargo run`.

Check the Swagger UI at `http://localhost:3000/open-api`.