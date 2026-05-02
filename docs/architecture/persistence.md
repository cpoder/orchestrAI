# Persistence

Branchwork keeps state in three layers: SQLite databases, a YAML plans
directory, and per-agent sibling files next to each session socket. This
page is the reference behind the headline "what survives a crash" table
in [overview.md](overview.md): every table, every on-disk artifact, the
migration model, and the four-restart-mode matrix at the bottom.

For schema lookups, the source of truth is
[`server-rs/src/db.rs`](../../server-rs/src/db.rs) (server) and
[`server-rs/src/saas/outbox.rs`](../../server-rs/src/saas/outbox.rs)
(runner + outbox). For wire formats see
[protocols.md](protocols.md); for the binaries that read and write these
artifacts see [server.md](server.md), [runner.md](runner.md), and
[session-daemon.md](session-daemon.md).

## Storage at a glance

```
host running branchwork-server
├── ~/.claude/
│   ├── plans/<slug>.yaml          # plan source of truth
│   ├── branchwork.db              # SQLite — agents, plans, auth, audit, outbox
│   └── sessions/
│       └── <agent-id>.{sock,log,pid,mcp.json}
└── <project-dir>/.git/refs/heads/branchwork/<plan>/<task>
                                   # task and fix branches

customer host running branchwork-runner (SaaS only)
├── ~/.branchwork-runner/
│   └── runner.db                  # SQLite — runner_outbox + seq_tracker
└── <runner-cwd>/.branchwork-runner-sessions/
    └── <agent-id>.{sock,log,pid,mcp.json}
```

`branchwork-server` and `branchwork-runner` never share a database.
Reliable wire messages between them ride the outbox tables on **both**
sides; everything else (PTY bytes, hook events, agent rows) is owned by
exactly one side.

---

## SQLite databases

### Server: `~/.claude/branchwork.db`

Default path is `<claude_dir>/branchwork.db`, where `<claude_dir>`
defaults to `~/.claude` and is overridden by `--claude-dir` (see
[`config.rs`](../../server-rs/src/config.rs)). The Helm chart mounts
`/data` in its place. The file is opened by [`db::init`] with
`journal_mode=WAL` and `foreign_keys=ON`, then [`migrate`] runs.

[`db::init`]: ../../server-rs/src/db.rs
[`migrate`]: ../../server-rs/src/db.rs

#### Plan and task state

| Table | Holds |
|---|---|
| `task_status` | `(plan_name, task_number)` PK → `status` (`pending` / `in_progress` / `completed` / `skipped` / `checking` / `failed`), `source` (`auto` / `manual` / `NULL` legacy), `updated_at`. The dashboard's done-count gate reads this; auto-status writes only with `source='auto'` and never overwrites `'manual'`. |
| `task_learnings` | Append-only log of free-text learnings recorded by agents (and the post-task curl in `build_task_prompt`). Read most-recent-first via `db::task_learnings`. |
| `plan_project` | `plan_name` PK → absolute path of the project repo on disk. Written by the New Plan flow and the project picker. |
| `plan_verdicts` | `plan_name` PK → output of the most recent plan-level Check agent: `verdict`, `reason`, `agent_id`, `checked_at`. |
| `plan_budget` | `plan_name` PK → `max_budget_usd`. Per-plan kill switch driven by the cost broadcast. |
| `plan_auto_advance` | `plan_name` PK → `enabled` flag for auto-spawning the next task on completion. |
| `plan_org` | `plan_name` PK → `org_id`. Authoritative plan-to-org mapping; plans without a row default to `'default-org'` for backward compat. |

Plans themselves are **not** in SQLite — they live as YAML files (see
[Plan files](#plan-files-claudeplans)). These tables hold runtime state
*about* plans only.

#### Agents and PTY transcript

| Table | Holds |
|---|---|
| `agents` | One row per spawned agent. Key columns: `id` (UUID PK), `mode` (`pty` / stream-json), `status` (`starting` / `running` / `completed` / `failed`), `pid`, `supervisor_socket`, `branch`, `source_branch`, `base_commit`, `prompt`, `driver`, `stop_reason` (`completed` / `killed` / `orphaned` / `supervisor_unreachable`), `cost_usd`, `org_id`, `user_id`. Drives the dashboard agent panel and is the canonical "is this agent live?" record. |
| `agent_output` | Append-only PTY chunks. Every `Output` frame the server reads from a session daemon is mirrored here as `(agent_id, message_type='pty', content)`. The `/terminal` WebSocket replays this table on reconnect, and `parse_cost_from_pty_output` reads the most recent rows when the driver advertises stream-json cost extraction. **This is the durable transcript the server holds**; the daemon's own `<socket>.log` is the authoritative one for post-mortem (see [Per-agent sibling files](#per-agent-sibling-files)). |
| `hook_events` | Inbound `POST /hooks` payloads from Claude Code's hook config. `(session_id, hook_type, tool_name, tool_input, timestamp)`. Indexed by `session_id` and `hook_type`. |

#### CI

| Table | Holds |
|---|---|
| `ci_runs` | One row per discovered CI run. `(plan_name, task_number)`, `commit_sha`, `branch`, `provider='github'`, `run_id`, `run_url`, `status`, `conclusion`, `failure_log` (cached `gh run view --log-failed` tail, ~8 KB), `dismissed_at` (soft-delete for the "X" button on the CI badge). The background `ci::spawn_poller` writes this; Fix-CI reads it. |

#### Auth, organizations, and SSO

| Table | Holds |
|---|---|
| `users` | `id` PK, `email` (UNIQUE), `password_hash` (bcrypt), `created_at`. |
| `sessions` | Cookie token PK, `user_id`, `created_at`, `expires_at`. ON DELETE CASCADE from `users`. |
| `organizations` | `id` PK, `name`, `slug` (UNIQUE), `created_at`. The default `'default-org'` is auto-seeded by `auth::orgs::ensure_default_org` at the end of every migration so a fresh self-hosted DB and a multi-tenant SaaS DB share the same shape. |
| `org_members` | `(org_id, user_id)` PK, `role` (`'member'` / `'admin'` / …), `joined_at`. |
| `sso_providers` | Per-org OIDC/SAML config: `protocol`, `enabled`, `email_domains`, `issuer_url`, `client_id`, `client_secret`, SAML `idp_*` / `sp_entity_id`, `groups_claim`, `group_role_mapping`. |
| `sso_accounts` | External-IdP identity rows linking `(provider_id, external_id)` → `user_id`. Carries the most recent `groups` claim and `last_login_at`. |
| `sso_auth_state` | Short-lived OIDC `state` / PKCE / nonce holder. TTL is the operator's responsibility today (no GC task — see [troubleshooting](../troubleshooting.md) once it lands). |

#### SaaS billing

| Table | Holds |
|---|---|
| `org_budgets` | `org_id` PK, `max_budget_usd`, `billing_period` (`'monthly'` …), `period_start`. |
| `user_quotas` | `(org_id, user_id)` PK, `max_budget_usd`. Per-user spend cap inside an org. |
| `budget_alerts` | `(org_id, threshold, period_key)` UNIQUE — once-per-period dedup so a 50 % / 80 % / 100 % alert fires exactly once per billing period. |
| `org_kill_switch` | `org_id` PK, `active` flag, `reason`. When active, `POST /api/agents` is rejected for that org regardless of budget. |

These are SaaS-only in practice — self-hosted dashboards have a single
`'default-org'` and never write to them — but the schema is identical
in both deployments.

#### Audit

| Table | Holds |
|---|---|
| `audit_logs` | `(org_id, user_id, user_email, action, resource_type, resource_id, diff, created_at)`. Indexed on `(org_id, created_at DESC)`, `action`, `(resource_type, resource_id)`. Every state-changing handler under `api/` and `auth/` calls `audit::record_action`. |

#### Remote runners and SaaS outbox

| Table | Holds |
|---|---|
| `runners` | `id` PK (`runner-<uuid>`), `name`, `org_id`, `status` (`online` / `offline`), `hostname`, `version`, `last_seen_at`, `created_at`. The in-memory `RunnerRegistry` on `AppState` is rebuilt at boot from this and from live WS connections. |
| `runner_tokens` | `token_hash` PK (despite the name, currently stores the plaintext 256-bit hex token — runner tokens are high-entropy random and bcrypt is not warranted; see the comment on `sha256_hex` in [`runner_ws.rs`](../../server-rs/src/saas/runner_ws.rs) and [runner.md](runner.md#authenticated-websocket-handshake)), `runner_name`, `org_id`, `created_by`, `created_at`. |
| `inbox_pending` | Server-side outbox for SaaS → runner reliable commands. `seq` PK AUTOINC, `runner_id`, `command_type`, `payload` (JSON), `acked`. Indexed on `(runner_id, acked)`. Created by `outbox::init_server_inbox` from inside `migrate()`. |
| `seq_tracker` | `peer_id` PK → `last_seq`. The dedup high-water mark per peer (see [`outbox::advance_peer_seq`](../../server-rs/src/saas/outbox.rs)). The server uses `peer_id = <runner_id>`; the runner uses `peer_id = "server"`. |

The `seq_tracker` table is **also** how the runner persists its own
runner_id across restarts: rows whose `peer_id` matches `"runner-%"`
are treated as the runner's stable identity (see
[runner.md § Runner ID persistence](runner.md#runner-id-persistence)).

### Runner: `~/.branchwork-runner/runner.db`

Defaults to `~/.branchwork-runner/runner.db`; overridable with
`--db-path`. Opened with `journal_mode=WAL`. Two tables, both created
on every startup:

| Table | Holds |
|---|---|
| `runner_outbox` | `seq` PK AUTOINC, `event_type`, `payload` (JSON), `created_at`, `acked`. Single sender (the runner), so no `runner_id` column. Created by `outbox::init_runner_outbox`. |
| `seq_tracker` | `peer_id` PK → `last_seq`. Stores both the runner-side high-water mark for messages received from `peer_id = "server"` and the runner's own `runner-<uuid>` identity row (read by `load_or_generate_runner_id`). |

There is no `agents` table on the runner — agent state is always
demanded from the dashboard. If the runner crashes and restarts, it
remembers nothing about the daemons it had spawned (see
[runner.md § Runner crash / SIGKILL](runner.md#runner-crash--sigkill)).

### Postgres mode

The Helm chart exposes `database.mode: sqlite | postgres` and wires
`DATABASE_URL` into the pod when `mode: postgres`, **but the Rust
binary itself only speaks SQLite**: nothing in `db.rs` reads
`DATABASE_URL`, and there is no Postgres driver linked. The Helm
postgres path is a deployment-template stub for a future migration —
setting it today produces a server that ignores the env var and still
opens `<claude_dir>/branchwork.db` on the (in that case un-mounted)
container filesystem.

The `sqlite` mode is also the only one that mounts a PVC: the chart's
`pvc.yaml` is gated on both `persistence.enabled` *and* `mode=sqlite`.
Until the Rust code grows a real Postgres backend, treat
`mode: postgres` as not-yet-implemented and stay on sqlite.

### Schema migrations

The migration model is intentionally low-tech: there is no version
table and no migration directory. `migrate()` is a single function in
[`db.rs`](../../server-rs/src/db.rs) that contains:

1. A `CREATE TABLE IF NOT EXISTS …` block for every table that has
   ever existed.
2. `init_server_inbox` and `init_seq_tracker` calls into the outbox
   module (same `IF NOT EXISTS` discipline).
3. A series of `ALTER TABLE … ADD COLUMN …` statements for every
   column added after a table was first shipped. Every one is wrapped
   in `.ok()` so duplicate-column errors on already-migrated DBs are
   silently ignored.
4. `auth::orgs::ensure_default_org` to seed the `'default-org'` row
   and migrate any orphaned `users`/`plans` into it.
5. `cleanup_stale_auto_completed` — a one-time-ish purge of legacy
   bulk auto-inferred `completed` rows from before the navbar
   false-completion fix; naturally idempotent post-Task-2.2 because no
   new row can satisfy the predicate.

This means **every server boot runs the full migration**, the
operation is idempotent, and there is no rollback / down-migration
story. To roll back a column you delete it, deploy, and accept that
older binaries will silently re-add it on their next boot.

The notable consequence: **adding a column is always
backwards-compatible** (older code ignores the new column; new code
sees `NULL` from older rows), but **renaming or removing a column is
not** — it requires writing the rename as an additive ALTER plus a
data backfill, then deleting the old column in a later release once
all readers have been upgraded.

The runner DB has the same model: open + `init_runner_outbox` +
`init_seq_tracker` on every boot.

---

## Plan files: `~/.claude/plans/`

```
~/.claude/plans/
├── <plan-slug>.yaml      # canonical
└── <plan-slug>.md        # legacy / authored-as-Markdown
```

Plans are the **only** source of truth for plan structure (phases,
tasks, descriptions, file lists, verification block). The SQLite
tables above hold runtime state *about* the plan — done-counts, agent
runs, verdicts — but never the plan tree itself. A `git checkout` that
rolls a plan file back will roll back the structure visible in the
dashboard the next time the file watcher fires.

The watcher is `file_watcher::start` (see [server.md](server.md)). It
debounces filesystem events on `*.md` / `*.yaml` / `*.yml` and emits a
`plan_updated` broadcast so the SPA refetches.

`update_plan` re-serializes via `serde_yaml` from `ParsedPlan`, so
unknown top-level keys round-trip but two known fields currently get
clobbered on UI saves: `verification` and per-task `produces_commit`
(both tracked in [design-produces-commit.md](../design-produces-commit.md)).
If you have a hand-edited YAML you don't want clobbered, edit it
directly and let the file watcher pick it up — don't round-trip
through the dashboard's edit forms.

In SaaS the plans directory lives on the **server's** disk, and remote
runners reach it indirectly via the MCP tunnel (the runner forwards
`list_plans` / `get_plan` calls into the dashboard's MCP surface). The
runner machine has no plans directory of its own.

---

## Per-agent sibling files

For every PTY-mode agent the supervisor writes four files next to its
local socket:

| Path | Owner | Lifetime |
|---|---|---|
| `<socket>` | session_daemon (listener) | Created on bind. On Unix, unlinked on clean shutdown — a SIGKILLed daemon leaves the socket file dangling, which is harmless because the next bind clears stale paths before listening. On Windows, named pipes are not file-system-visible; nothing to clean up. |
| `<socket>.pid` | session_daemon | Written after `setsid` / `DETACHED_PROCESS` so the spawning parent can read the durable post-fork PID. Removed as the **last** step of clean shutdown. **Presence after the socket goes silent ⇒ supervisor crashed**, which is the canonical signal `pty_agent::on_agent_exit` uses to write `status='failed', stop_reason='supervisor_unreachable'`. Faster than waiting for the 45 s heartbeat. |
| `<socket>.log` | session_daemon | Open-append for the daemon's lifetime, fsynced per chunk. **Survives the daemon's exit.** This is the post-mortem record of every byte the PTY emitted. Note: the `agent_output` SQLite table is what the dashboard's `/terminal` WS replays for browsers — `<socket>.log` is the durable on-disk version that reaches further back than the server's view (it captures bytes emitted while the server was offline). |
| `<socket>.mcp.json` | spawning server / runner | Written by `start_pty_agent` when the driver advertises an MCP config (Claude today). Read by the CLI as `--mcp-config <path>`. Stays on disk after the agent exits and is overwritten by the next agent that lands on the same id. |

Default location:

- **Self-hosted server:** `<claude_dir>/sessions/<agent-id>.{sock,log,pid,mcp.json}`.
- **SaaS runner:** `<runner-cwd>/.branchwork-runner-sessions/<agent-id>.{sock,log,pid,mcp.json}`.

The directory is created lazily on first spawn (`create_dir_all`).
Nothing prunes old transcripts; an operator who wants to bound disk
must do it externally (logrotate, a cron, or a manual sweep).

For the full lifecycle and replay semantics see
[session-daemon.md](session-daemon.md).

---

## Project git worktrees

Every agent runs on a dedicated branch in the project repo:

```
branchwork/<plan-slug>/<task-number>          # task agents
branchwork/fix/<plan>/<task>/<run-id>         # Fix-CI recovery agents
```

These are normal git refs in the project's own `.git/`. The dashboard
reads them but does not own them — `git push`, `git fetch`, and
worktree health are the user's (or CI's) responsibility. The
`agents.branch` / `agents.source_branch` / `agents.base_commit`
columns hold the SHAs the dashboard captured at spawn time so it can
reason about empty branches, staleness, and the merge button gate
without re-walking git history on every render.

In SaaS, project git lives on the **runner's** machine; the dashboard
server never touches the customer's repo directly. Merges are
performed by the runner on instruction.

---

## What survives what kind of restart

The acceptance criterion for this page: every persisted artifact × all
four failure modes. Y = survives untouched, R = survives but is
reconciled at boot, N = lost.

| Artifact | Server restart | Runner restart (SaaS) | session_daemon kill | Machine reboot |
|---|---|---|---|---|
| `~/.claude/branchwork.db` (server SQLite file) | Y — WAL + fsync | Y (different host) | Y | Y |
| `~/.branchwork-runner/runner.db` (runner SQLite) | Y (different host) | Y — WAL + fsync | Y | Y |
| `~/.claude/plans/*.yaml` | Y | Y | Y | Y |
| `agents` rows (`status`, `branch`, `cost_usd`, …) | R — `cleanup_and_reattach` either reattaches (PTY mode, PID alive, socket reachable) or marks `failed` + `stop_reason='orphaned'` and broadcasts `agent_stopped`. Stream-JSON rows always orphan. | Y on the server side; runner-side in-memory `state.agents` is N (no reattach analogue today — see [runner.md § Runner crash / SIGKILL](runner.md#runner-crash--sigkill)). | R — `pty_agent::on_agent_exit` flips the row to `failed`+`supervisor_unreachable` if `<socket>.pid` is still present, `completed` if not. | R — every PTY-mode row will fail reattach (PIDs are gone) and orphan; user re-Starts. |
| `agent_output` rows (server-captured PTY transcript) | Y up to the crash; the gap between crash and reattach is **not** rewritten retrospectively from `<socket>.log` (a yellow banner is injected on the next `/terminal` connect). | Y on the server. | Y. | Y up to the reboot. |
| `task_status`, `task_learnings`, `plan_verdicts` | Y | Y | Y | Y |
| `ci_runs` (incl. `failure_log`, `dismissed_at`) | Y | Y | n/a | Y |
| `audit_logs` | Y | Y | Y | Y |
| `users`, `sessions`, `sso_*` | Y. Active browser cookies stay valid until `expires_at`. | Y | Y | Y |
| `runners` rows | Y; affected rows flip to `status='offline'` until the runner reconnects. | Y; flipped to `offline` then back to `online` on reconnect. | n/a | Y; `offline` until each runner reboots and reconnects. |
| `inbox_pending` (server-side outbox: SaaS → runner) | Y. Unacked rows replay on next runner connect after the server's `Resume`. | Y. Unacked rows replay when the runner reconnects. | n/a | Y. Replays after both sides come back. |
| `runner_outbox` (runner-side outbox: runner → SaaS) | Y; replays after the server is back. | Y; replays on reconnect. | n/a | Y; replays after reboot + reconnect. |
| `seq_tracker` (peer high-water marks **and** runner_id row) | Y | Y. **Deleting `runner.db` resets the runner_id** — the server sees a fresh runner appear and the old one stays permanently `offline`. | n/a | Y |
| In-flight reliable WS frames between runner and server | R — durable in `runner_outbox` / `inbox_pending`; replayed on reconnect with the same seqs. The receiver dedups via `advance_peer_seq`. | R — same path. | n/a | R — same path. |
| In-flight best-effort WS frames (`AgentOutput`, `AgentInput`, `Ping`, `Pong`) | N — dropped. The dashboard catches up to live output on the next `AgentOutput` frame; `TerminalReplay` covers byte-range backfill from `<socket>.log` for browsers that reconnect. | N — dropped. | n/a | N — dropped. |
| Live PTY / CLI process | Y — the daemon is reparented to PID 1 by `setsid` (Unix) or already detached via `DETACHED_PROCESS` (Windows). The CLI keeps running while the server is down. | Y — same. | **N — the CLI dies with the daemon.** This is the only "the agent's process actually died" cell in the matrix. | N — kernel restart wipes everything. |
| Local IPC socket (`<socket>`) | Y — daemon still owns it. | Y — daemon still owns it. | Stale path on Unix until the next bind clears it; Windows pipes vanish with the daemon. | N — daemon is gone. |
| `<socket>.pid` | Y; presence is the next-server-boot's crash signal. | Y. | Y if the daemon was SIGKILLed (this is how `on_agent_exit` detects the crash); N on clean exit. | Y across the reboot — the next server boot will read it, fail reattach (PID is dead), and orphan the row. The pidfile is then removed by `on_agent_exit` (which is what makes that path idempotent). |
| `<socket>.log` (daemon-side PTY transcript) | Y. | Y. | Y — the log is fsynced per chunk. | Y. |
| `<socket>.mcp.json` | Y. | Y. | Y. | Y. |
| Project git branches `branchwork/<plan>/<task>` | Y. | Y. | Y. | Y. |
| Dashboard WebSocket subscribers (`/ws`) | N — every browser tab reconnects on its own; on reconnect the SPA refetches plans/agents and gets the new state from the SQLite-backed REST surface. | n/a | n/a | N — same. |
| In-flight HTTP requests | N — clients retry. | n/a | n/a | N — clients retry. |

Two invariants the matrix relies on:

1. **Daemons outlive their parent.** Self-hosted server crashes, SaaS
   runner crashes, and `systemctl restart branchwork-server` all leave
   each per-agent session_daemon running. Detach is at spawn time
   ([`supervisor::detach_from_parent`](../../server-rs/src/agents/supervisor.rs)).
   The only way the in-PTY CLI process dies is if you kill the daemon
   itself, the OS kills the daemon, or the box reboots.
2. **Reliable wire frames are durable on both sides.** The outbox
   tables sit on disk in `branchwork.db` (server) and `runner.db`
   (runner); the seq + always-ACK-on-dup + per-peer dedup combination
   in `outbox::advance_peer_seq` makes the failure modes above
   exactly-once-for-side-effects from the receiver's point of view
   even though the wire is at-least-once.

What does **not** persist anywhere is anything held only in memory:
the `RunnerRegistry` map, the `AgentRegistry`'s in-process
`ManagedAgent` channels, the broadcast channel buffer, the runner's
in-memory `state.agents` map, and the `tokio::sync::broadcast`
buffer inside each session daemon. Everything user-visible is
reconstructable from disk; only "the few hundred terminal characters
in flight at the moment of the crash" is irrecoverable, which is why
high-frequency I/O is explicitly best-effort.

---

## See also

- [overview.md](overview.md) — three-binary diagram and the "What
  persists across what kind of restart" headline table.
- [server.md](server.md) — `db::init`, `cleanup_and_reattach`, and how
  the dashboard wires the SQLite layer into request handlers.
- [session-daemon.md](session-daemon.md) — `<socket>.{sock,log,pid,mcp.json}`
  ownership, replay semantics, and the pidfile crash signal.
- [runner.md](runner.md) — runner DB initialization, runner_id
  persistence, outbox / replay, and the `Runner crash / SIGKILL` gap.
- [protocols.md](protocols.md) — wire formats whose durability
  semantics this page describes.
- [`server-rs/src/db.rs`](../../server-rs/src/db.rs),
  [`server-rs/src/saas/outbox.rs`](../../server-rs/src/saas/outbox.rs),
  [`deploy/helm/branchwork/values.yaml`](../../deploy/helm/branchwork/values.yaml)
  — canonical implementation.
