# orchestrAI

Real-time web dashboard for visualizing Claude Code plans, agents, and tasks.

orchestrAI watches your `~/.claude` directory for plan files and hook events, displays them in a live dashboard, and lets you manage agent sessions — all from a single self-contained binary.

## Build from source

Requires Rust 1.85+, Node.js 20+, and pnpm.

```sh
# Build frontend
pnpm --filter @orchestrai/web build

# Build server (embeds frontend)
cd server-rs && cargo build --release
```

Binary: `server-rs/target/release/orchestrai-server`

## Usage

```sh
orchestrai-server [OPTIONS]
```

| Flag           | Default     | Description                                          |
|----------------|-------------|------------------------------------------------------|
| `--port`       | `3100`      | HTTP port                                            |
| `--effort`     | `high`      | Effort level for agents (`low`, `medium`, `high`, `max`) |
| `--claude-dir` | `~/.claude` | Path to `.claude` directory                          |

Open `http://localhost:3100` in your browser.

## Project structure

```
orchestrAI/
  server-rs/      Rust server (Axum, rusqlite, portable-pty, tmux)
  web/            React frontend (Vite, Tailwind, xterm.js)
```

## License

MIT
