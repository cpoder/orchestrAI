//! Integration tests for `GET /api/plans` `doneCount` accuracy.
//!
//! Regression coverage for the navbar false-positive bug: a plan whose
//! tasks all have existing `file_paths` used to flip to `completed` via
//! the auto-status heuristic and inflate `doneCount` to the task count,
//! moving the plan into the Done section without any real work.
//!
//! Post-fix invariant: auto-status alone never inflates `doneCount`
//! (it caps at `in_progress`), and a manual `update_task_status` call
//! is the only path that increments it.

mod support;

use serde_json::json;
use support::TestDashboard;

/// 3-task plan where every task points at one file. Project is the
/// absolute scratch repo path (see `recovery.rs::minimal_plan` for the
/// rationale on absolute paths under Windows).
///
/// Built via `format!` with a raw string instead of `\<newline>`
/// continuations: the latter strips ALL leading whitespace on the next
/// line, which destroys YAML indentation and trips
/// `phases[0]: missing field \`title\`` in serde_yaml.
fn three_task_plan(name: &str, project_dir: &std::path::Path) -> String {
    format!(
        r#"title: {name}
context: ''
project: {project}
phases:
  - number: 1
    title: Phase 1
    description: ''
    tasks:
      - number: '1.1'
        title: Task one
        description: ''
        file_paths:
          - src/a.rs
        acceptance: ''
      - number: '1.2'
        title: Task two
        description: ''
        file_paths:
          - src/b.rs
        acceptance: ''
      - number: '1.3'
        title: Task three
        description: ''
        file_paths:
          - src/c.rs
        acceptance: ''
"#,
        name = name,
        project = project_dir.display()
    )
}

/// Look up our plan in the `GET /api/plans` response and return its
/// `(taskCount, doneCount)`. Asserts the plan was found.
fn fetch_counts(d: &TestDashboard, name: &str) -> (u64, u64) {
    let (s, body) = d.get("/api/plans");
    assert_eq!(s, 200, "GET /api/plans: {body}");
    let entry = body
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("plan {name} missing from /api/plans response: {body}"));
    (
        entry["taskCount"].as_u64().unwrap(),
        entry["doneCount"].as_u64().unwrap(),
    )
}

#[test]
fn auto_status_alone_does_not_inflate_done_count() {
    let d = TestDashboard::new();

    // Commit the three files referenced by the plan so they exist on
    // disk *and* the working tree stays clean — the latter matters
    // because `set_task_status(completed)` is gated on a clean tree.
    let src = d.project.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for f in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(src.join(f), "// placeholder\n").unwrap();
    }
    std::process::Command::new("git")
        .args(["add", "src"])
        .current_dir(&d.project)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "seed task files"])
        .current_dir(&d.project)
        .status()
        .unwrap();

    let plan = d.create_plan(
        "done-count-plan",
        &three_task_plan("done-count-plan", &d.project),
    );

    // Pre-condition: no DB rows yet — doneCount=0, taskCount=3.
    let (task_count, done_count) = fetch_counts(&d, &plan);
    assert_eq!(task_count, 3);
    assert_eq!(done_count, 0);

    // Run the auto-status heuristic. Every task's file exists, so the
    // pre-fix code would have inferred `completed` for all three and
    // pushed doneCount to 3. Post-fix, infer_status caps at
    // `in_progress`, which `doneCount` (completed|skipped only) ignores.
    let (s, body) = d.post(&format!("/api/plans/{plan}/auto-status"), json!({}));
    assert_eq!(s, 200, "auto-status: {body}");
    let summary = &body["summary"];
    assert_eq!(
        summary["completed"], 0,
        "auto-status must never produce completed: {body}"
    );
    assert_eq!(
        summary["in_progress"], 3,
        "all three files exist, so all three tasks should be in_progress: {body}"
    );

    let (_, done_count) = fetch_counts(&d, &plan);
    assert_eq!(
        done_count, 0,
        "auto-inferred in_progress must not contribute to doneCount"
    );

    // Manually flip one task to completed via the same endpoint the
    // dashboard / MCP `update_task_status` tool calls.
    let (s, body) = d.put(
        &format!("/api/plans/{plan}/tasks/1.1/status"),
        json!({"status": "completed"}),
    );
    assert_eq!(s, 200, "set_task_status(completed): {body}");

    let (_, done_count) = fetch_counts(&d, &plan);
    assert_eq!(
        done_count, 1,
        "manual completion of one task must bump doneCount to 1"
    );
}
