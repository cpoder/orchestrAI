//! End-to-end tests for the merge guard and related branch handling.
//!
//! These codify the bugs I shipped in dashboard-polish so they can't
//! regress:
//!   - Empty branch merge must 409, not silently no-op.
//!   - An agent whose source_branch equals the task branch (stale
//!     checkout) must not make the merge guard hit a self-compare.
//!   - rev-list errors on stale refs must degrade permissively, not 500.

mod support;

use serde_json::json;
use support::TestDashboard;

fn seed_agent(
    d: &TestDashboard,
    id: &str,
    plan: &str,
    task: &str,
    branch: Option<&str>,
    source_branch: Option<&str>,
) {
    let conn = rusqlite::Connection::open(d.dir.path().join(".claude/branchwork.db")).unwrap();
    conn.execute(
        "INSERT INTO agents \
            (id, session_id, cwd, status, mode, plan_name, task_id, \
             branch, source_branch) \
         VALUES (?1, ?1, ?2, 'completed', 'pty', ?3, ?4, ?5, ?6)",
        rusqlite::params![
            id,
            d.project.to_string_lossy(),
            plan,
            task,
            branch,
            source_branch
        ],
    )
    .unwrap();
}

/// Plan YAML with an absolute `project` path — Windows' `dirs::home_dir()`
/// ignores env overrides, so we can't rely on `$HOME/<name>`. Absolute
/// paths work on both platforms.
fn minimal_plan(name: &str, project_dir: &std::path::Path) -> String {
    format!(
        "title: {name}\ncontext: ''\nproject: {project}\nphases:\n  - number: 1\n    title: Phase 1\n    description: ''\n    tasks:\n      - number: '1.1'\n        title: Task 1.1\n        description: ''\n        acceptance: ''\n",
        name = name,
        project = project_dir.display()
    )
}

#[test]
fn empty_branch_merge_returns_409_not_500() {
    let d = TestDashboard::new();
    d.create_plan("mp-a", &minimal_plan("mp-a", &d.project));

    // A branch with no commits ahead of master — the classic "agent
    // exited without committing" failure mode. Before the guard this
    // silently no-opped. Now it should 409.
    let empty = "branchwork/mp-a/1.1";
    d.create_task_branch(empty, /* with_commit */ false);
    seed_agent(
        &d,
        "agent-empty",
        "mp-a",
        "1.1",
        Some(empty),
        Some("master"),
    );

    let (s, body) = d.post("/api/agents/agent-empty/merge", json!({}));
    assert_eq!(s, 409, "expected 409, got {s}: {body}");
    let msg = body["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("no commits") || msg.contains("not committed"),
        "error message should mention missing commits: {msg}"
    );
    // Branch should still exist — we didn't delete it.
    assert!(d.local_branches().contains(&empty.to_string()));
}

#[test]
fn merge_with_real_commits_succeeds() {
    let d = TestDashboard::new();
    d.create_plan("mp-b", &minimal_plan("mp-b", &d.project));

    let br = "branchwork/mp-b/1.1";
    d.create_task_branch(br, /* with_commit */ true);
    seed_agent(&d, "agent-real", "mp-b", "1.1", Some(br), Some("master"));

    let (s, body) = d.post("/api/agents/agent-real/merge", json!({}));
    assert_eq!(s, 200, "expected 200, got {s}: {body}");
    assert_eq!(body["ok"], true);
    // The branch gets deleted after successful merge.
    assert!(!d.local_branches().contains(&br.to_string()));
}

#[test]
fn self_referencing_source_branch_does_not_cause_500() {
    // Regression: b77d9c0 shipped Fix CI with source_branch recorded AS
    // the task branch (because start_pty_agent captured git_current_branch
    // on a freshly-checked-out task branch). The merge guard then
    // compared X..X = 0 commits and 409'd every legitimate merge; when
    // git couldn't resolve the range it 500'd with "ambiguous argument".
    // The fix (3fea12d) stores NULL when source == task_branch and
    // degrades permissively when rev-list errors.
    let d = TestDashboard::new();
    d.create_plan("mp-c", &minimal_plan("mp-c", &d.project));

    let br = "branchwork/mp-c/1.1";
    d.create_task_branch(br, /* with_commit */ true);
    // Store source_branch as the same thing as branch — the bug shape.
    seed_agent(&d, "agent-self", "mp-c", "1.1", Some(br), Some(br));

    let (s, body) = d.post("/api/agents/agent-self/merge", json!({}));
    // Accept either: 200 (guard falls back to main/master and merge
    // succeeds) or 409 with a legible error. Absolutely no 500.
    assert!(
        s == 200 || s == 409,
        "expected 200 or 409 (not 500), got {s}: {body}"
    );
    // If merge succeeded, the branch is gone. If 409, the branch remains.
    if s == 200 {
        assert!(!d.local_branches().contains(&br.to_string()));
    }
}

#[test]
fn merge_with_nonexistent_source_branch_does_not_500() {
    // If source_branch points at a deleted ref (out-of-band cleanup),
    // the rev-list call errors. Before the fix the guard returned 500.
    // Now it logs and falls through to `git merge` which has its own
    // clearer error handling.
    let d = TestDashboard::new();
    d.create_plan("mp-d", &minimal_plan("mp-d", &d.project));

    let br = "branchwork/mp-d/1.1";
    d.create_task_branch(br, /* with_commit */ true);
    seed_agent(
        &d,
        "agent-ghost",
        "mp-d",
        "1.1",
        Some(br),
        Some("refs/no/such/branch"),
    );

    let (s, _body) = d.post("/api/agents/agent-ghost/merge", json!({}));
    // No 500 — accept 200/409/500-if-the-actual-merge-fails-loudly (not
    // from the guard). Specifically test: not a 500 from the "Failed to
    // inspect branch commits" path.
    assert!(
        s != 500,
        "guard must not 500 on unresolvable source_branch (got {s})"
    );
}

/// Reproduce the bug behind plan
/// `merge-target-canonical-default-branch`: when an agent's
/// `source_branch` points at a *resolvable but stale* branch (e.g. the
/// user was on a feature branch when they started the agent, so
/// `git_current_branch` captured that name instead of master), the
/// merge button lands the task commit on the stale branch and master
/// never sees the work.
///
/// Setup:
/// - master is at the initial commit.
/// - architecture-docs/3.4 was branched off master with one extra
///   commit, simulating an old base branch the user once worked on.
/// - branchwork/foo/1.1 was branched off architecture-docs/3.4 with
///   one task commit, so it fast-forwards onto the stale branch.
/// - The agents row records source_branch = "architecture-docs/3.4"
///   (the captured-at-spawn-time stale base).
///
/// Pre-fix behaviour, asserted here:
/// - `resolve_merge_target` returns "architecture-docs/3.4" because
///   it resolves; master fallback never kicks in.
/// - The merge succeeds (200), the response says
///   `into = "architecture-docs/3.4"`, the task commit fast-forwards
///   onto the stale branch, and master is untouched.
///
/// Marked `#[ignore]` so the buggy behaviour is documented but does
/// not gate CI. Phase 2 will change the target resolution to prefer
/// the canonical default branch; at that point this test should be
/// deleted or rewritten.
#[test]
#[ignore = "documents pre-fix behaviour for plan merge-target-canonical-default-branch — Phase 2 will change merge target"]
fn merge_lands_on_stale_source_branch_instead_of_master() {
    let d = TestDashboard::new();
    d.create_plan("foo", &minimal_plan("foo", &d.project));

    // architecture-docs/3.4: a stale base branch off master with one
    // distinct commit. Resolvable, so resolve_merge_target() prefers
    // it over the master fallback.
    git(&d.project, &["checkout", "-q", "-b", "architecture-docs/3.4"]);
    std::fs::write(d.project.join("arch.md"), "stale base").unwrap();
    git(&d.project, &["add", "arch.md"]);
    git(&d.project, &["commit", "-q", "-m", "stale base commit"]);

    // branchwork/foo/1.1: the task branch, descended from the stale
    // base with one task commit. This is a fast-forward onto
    // architecture-docs/3.4.
    git(&d.project, &["checkout", "-q", "-b", "branchwork/foo/1.1"]);
    std::fs::write(d.project.join("work.txt"), "task work").unwrap();
    git(&d.project, &["add", "work.txt"]);
    git(&d.project, &["commit", "-q", "-m", "task work commit"]);
    let task_sha = head_sha(&d.project);

    git(&d.project, &["checkout", "-q", "master"]);
    let master_sha_before = head_sha(&d.project);

    seed_agent(
        &d,
        "agent-stale-target",
        "foo",
        "1.1",
        Some("branchwork/foo/1.1"),
        Some("architecture-docs/3.4"),
    );

    let (s, body) = d.post("/api/agents/agent-stale-target/merge", json!({}));
    assert_eq!(s, 200, "expected 200, got {s}: {body}");
    assert_eq!(
        body["into"], "architecture-docs/3.4",
        "merge landed on the wrong target: {body}"
    );
    assert_eq!(
        body["merged"], "branchwork/foo/1.1",
        "expected the task branch to be the merged ref: {body}"
    );

    // architecture-docs/3.4 fast-forwarded to the task commit.
    assert_eq!(
        rev_parse(&d.project, "architecture-docs/3.4"),
        task_sha,
        "stale branch should now point at the task commit"
    );

    // Master is untouched — never saw the task commit nor the stale
    // base commit. This is the user-visible bug: the canonical
    // default branch is missing the work the user thought they merged.
    assert_eq!(
        rev_parse(&d.project, "master"),
        master_sha_before,
        "master must not have moved"
    );
    assert!(
        !std::path::Path::new(&d.project).join("work.txt").exists()
            || {
                // We're on master and work.txt only exists on the task
                // branch / stale branch. After the merge the server
                // checked out architecture-docs/3.4, so the working
                // tree may have work.txt — but master itself should
                // have no work.txt commit. Verify via ls-tree.
                let out = std::process::Command::new("git")
                    .args(["ls-tree", "-r", "--name-only", "master"])
                    .current_dir(&d.project)
                    .output()
                    .unwrap();
                let names = String::from_utf8_lossy(&out.stdout);
                !names.lines().any(|l| l == "work.txt")
            },
        "master tree must not contain the task's work.txt"
    );

    // Task branch was deleted by the success path.
    assert!(
        !d.local_branches().contains(&"branchwork/foo/1.1".to_string()),
        "task branch should have been deleted after merge: {:?}",
        d.local_branches()
    );
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    if !out.status.success() {
        panic!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn head_sha(cwd: &std::path::Path) -> String {
    rev_parse(cwd, "HEAD")
}

fn rev_parse(cwd: &std::path::Path, refname: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", refname])
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("spawn git rev-parse {refname}: {e}"));
    assert!(
        out.status.success(),
        "git rev-parse {refname}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
