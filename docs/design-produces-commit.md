# Design — per-task `produces_commit` field gates the merge button

Phase 1, Task 1.1 deliverable. Records the decisions for the new field
that lets a plan author declare a task is not expected to land a commit
(repro / investigation / design tasks), so the merge banner stops
offering a button that always 409s.

## Current merge-banner gate (confirmed)

`web/src/components/TaskCard.tsx:89-97` — `branchAgent` selector:

```ts
const branchAgent = agents.find(
  (a) =>
    a.plan_name === planName &&
    a.task_id === task.number &&
    a.branch &&
    a.status !== "running" &&
    a.status !== "starting"
);
```

`web/src/components/TaskCard.tsx:561` — banner JSX gate:

```tsx
{branchAgent?.branch && ( … indigo banner with Discard + Merge … )}
```

Three conditions today, all on the agent row, none on the task: an agent
exists for `(plan_name, task_id)`, `agent.branch` is non-null, and
`agent.status` is neither `running` nor `starting`. There is no per-task
signal that the task was never expected to commit — exactly the gap
documented in `docs/repro-stale-merge-button.md`.

## Decisions

### 1. Field name

`produces_commit` (snake_case) in plan YAML and on the Rust
`YamlPlanTask` struct. The `PlanTask` struct (which `serde(rename_all =
"camelCase")`s on the wire) exposes `producesCommit` to the frontend.

Rationale:
- `produces_commit` reads as a declarative property of the task ("does
  this task produce a commit?"), same shape as `dependencies`.
- `commits` (the alt named in the plan) is ambiguous — easy to misread
  as a count, a list of SHAs, or a boolean.
- Existing YAML keys are snake_case (`file_paths`, `dependencies`); the
  frontend already gets camelCase via the `PlanTask` derive.

### 2. Default when absent

`true`. Every existing plan and every task that does not opt in keeps
exactly today's behavior. Only tasks that explicitly say
`produces_commit: false` lose the Merge button. This is the
backwards-compatible default — no migration of existing plan YAMLs.

### 3. Serialization

On `YamlPlanTask` in `server-rs/src/plan_parser.rs`:

```rust
#[serde(default = "default_true", skip_serializing_if = "is_true")]
produces_commit: bool,

fn default_true() -> bool { true }
fn is_true(b: &bool) -> bool { *b }
```

- `default = "default_true"` — missing key in YAML deserializes to
  `true`, matching the default-when-absent decision.
- `skip_serializing_if = "is_true"` — when the server rewrites a plan
  YAML (PUT `/api/plans/:name`), `produces_commit: true` is omitted so
  existing plans round-trip unchanged. Only `produces_commit: false`
  appears in the YAML on disk.
- On `PlanTask` (the API/UI shape), use `#[serde(default =
  "default_true")]` so JSON callers without the field also get `true`,
  and `#[serde(skip_serializing_if = "is_true")]` to keep API payloads
  small. The frontend sees `producesCommit: false` only when the task
  opts out; `undefined` ≡ `true`.

### 4. Where the check lives in the UI

In `TaskCard.tsx:561` (the indigo banner JSX) — not in the
`branchAgent` selector. The agent row is unchanged; only the rendered
controls change.

When `task.producesCommit === false`:

- **Hide the Merge button only.** The Discard button stays so the
  branch can still be cleaned up. (An empty branch is exactly when
  Discard matters most.)
- **Swap the heading/intent copy.** The banner is currently implicit
  ("here is a branch ready to merge"). For non-commit tasks, change the
  visual cue to read as a review pane:
  - Banner background: keep indigo (still a branch present), but the
    rightmost action area shows only Discard.
  - Add a small label above the branch name: "Review artifacts" (vs.
    today's implicit "Ready to merge").
  - Tooltip on Discard for this case: "Delete branch `<name>` — this
    task was not expected to commit".

The truthiness check uses `task.producesCommit !== false` for the
Merge button (so `undefined` and `true` both render Merge; only the
explicit opt-out hides it). Use the same predicate for the heading
copy swap.

### 5. Server-side behavior

No change. The 409 guard at
`server-rs/src/api/agents.rs:302-353` (`merge_agent_branch` running
`git rev-list --count <target>..<task_branch>` before checkout) stays
in place as defense-in-depth: a `produces_commit: true` task whose
agent happens to exit without committing still gets the same 409.
Frontend hides the button proactively; backend rejects regardless.

## Files that will change in later tasks

For reference (not modified in 1.1):

- `server-rs/src/plan_parser.rs` — add `produces_commit` to
  `YamlPlanTask` and `PlanTask` with the serde attrs above; thread
  through `parse_plan_yaml`, `parse_plan_markdown` (default `true`),
  and the YAML-rewrite path in PUT `/api/plans/:name`.
- `web/src/stores/plan-store.ts` — add `producesCommit?: boolean` to
  the `PlanTask` TS type.
- `web/src/components/TaskCard.tsx:561` — gate the Merge button +
  swap heading copy.
- Plan YAML schema docs (if any) — note the new optional field.

## Acceptance

- [x] Field name decided: `produces_commit` (YAML/Rust),
      `producesCommit` (TS).
- [x] Default decided: `true` (preserves current behavior, no
      migration).
- [x] Serialization decided: serde `default = "default_true"` +
      `skip_serializing_if = "is_true"` on both `YamlPlanTask` and
      `PlanTask`.
- [x] UI behavior decided: hide Merge only, keep Discard, swap
      heading copy to "Review artifacts" when `producesCommit === false`.
- [x] Recorded here so the PR description can lift this section
      verbatim.
