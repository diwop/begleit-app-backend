---
trigger: always_on
---

# Running the Server

When starting or running the application server (e.g. via `cargo run`):

1. **Always Start Outside the Sandbox (`BypassSandbox: true`)**:
   - The server **MUST** always be executed with `BypassSandbox: true`.
   - The sandbox blocks outbound network access by default. Because the server connects to external LLM APIs (such as Google Gemini) during startup warmup readiness checks (`/readyz`) and runtime requests (`/translate`, `/correct-transcript`), running inside the sandbox causes outbound API calls and readiness checks to fail with network/connection errors.

2. **Worktree Port Configuration**:
   - The server reads its port from `.env` or the `PORT` environment variable (defaulting to `3000`).
   - Ensure the server uses the port configured in the worktree's `.env` (e.g. `PORT=3001`) to support running multiple worktrees concurrently without port collisions.

3. **Daemon / Long-Running Mode**:
   - When launching the server as a background process, set `IsDaemon: true` and specify a suitable `WaitMsBeforeAsync` (e.g. `500` to `2000`).
