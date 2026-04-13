# orchestrAI

Real-time web dashboard for visualizing Claude Code plans, agents, and tasks.

orchestrAI watches your `~/.claude` directory for plan files and hook events, displays them in a live dashboard, and lets you manage agent sessions -- all from a single self-contained binary.

## Installation

### Quick install (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/cpoder/orchestrAI/master/install.sh | sh
```

This downloads the latest release binary for your platform and places it in `/usr/local/bin`. Override the install location with `ORCHESTRAI_INSTALL_DIR`:

```sh
ORCHESTRAI_INSTALL_DIR=~/.local/bin sh install.sh
```

To install a specific version:

```sh
ORCHESTRAI_VERSION=v0.1.0 sh install.sh
```

### Prebuilt binaries

Download a tarball from the [Releases](https://github.com/cpoder/orchestrAI/releases) page. Binaries are available for:

| Platform       | Artifact                            |
|----------------|-------------------------------------|
| Linux x64      | `orchestrai-server-linux-x64`       |
| Linux arm64    | `orchestrai-server-linux-arm64`     |
| macOS x64      | `orchestrai-server-macos-x64`       |
| macOS arm64    | `orchestrai-server-macos-arm64`     |

### Build from source

Requires Rust 1.85+, Node.js 20+, and pnpm.

```sh
make build
```

The binary is at `server-rs/target/release/orchestrai-server`.

## Usage

```sh
orchestrai-server [OPTIONS]
```

| Flag           | Default          | Description                       |
|----------------|------------------|-----------------------------------|
| `--port`       | `3100`           | HTTP port to listen on            |
| `--effort`     | `high`           | Effort level for spawned agents (`low`, `medium`, `high`, `max`) |
| `--claude-dir` | `~/.claude`      | Path to the `.claude` directory   |

Open `http://localhost:3100` in your browser to access the dashboard.

## Development

```sh
# Start Node.js dev server + Vite (hot reload)
make dev

# Or run components individually:
make dev-web      # Vite dev server only
make dev-server   # Rust server only (cargo run)
```

## Migrating from Node.js to Rust

The server was rewritten from Node.js (Express/TypeScript) to Rust (Axum/Tokio). The Rust binary is a drop-in replacement -- no configuration changes needed.

### What changed

- **Server binary:** The Node.js server (`server/`) has been replaced by a single compiled binary (`orchestrai-server`) built from `server-rs/`.
- **Frontend:** The React/Vite frontend is now embedded directly in the binary via `rust-embed`. No separate static file serving required.
- **No runtime dependencies:** No Node.js, no `node_modules`, no `pnpm`. Just the binary.

### What stayed the same

- **SQLite database:** Same schema, same file (`~/.claude/orchestrai.db`). The Rust server reads and writes the same tables (`hook_events`, `agents`, `agent_output`, `plan_project`, `task_status`).
- **CLI flags:** `--port`, `--effort`, `--claude-dir` all work identically.
- **API endpoints:** All REST and WebSocket endpoints are unchanged.
- **Default port:** Still `3100`.

### Upgrade steps

1. Stop the running Node.js server.
2. Install the new binary (via the install script or a release download).
3. Start `orchestrai-server`. It picks up your existing database and plans automatically.

Your database and all plan/task state are preserved -- no migration script needed.

## Project structure

```
orchestrAI/
  server-rs/      Rust server (Axum, rusqlite, portable-pty)
  server/         Legacy Node.js server (to be removed)
  web/            React frontend (Vite, Tailwind, xterm.js)
  install.sh      Platform installer script
  Makefile         Build and dev commands
```

## License

MIT
