//! E2E tests for the Archive panel endpoints (plan-deletion 3.2):
//!
//! - `GET /api/snapshots` — list every `plan_snapshots` row scoped to
//!   the caller's org.
//! - `DELETE /api/snapshots/:id` — immediate hard-delete of a single
//!   snapshot row + its archive YAML, audited as
//!   `plan.snapshot_purged_manual`.
//!
//! These tests exercise the round-trip with a real server + SQLite and
//! pin the response shape, the audit row, and the cross-org gate.

mod support;

use rusqlite::params;
use support::TestDashboard;

fn minimal_plan(name: &str, project_dir: &std::path::Path) -> String {
    format!(
        "title: {name}\ncontext: ''\nproject: {project}\nphases:\n  - number: 1\n    title: Phase 1\n    description: ''\n    tasks:\n      - number: '1.1'\n        title: Task 1.1\n        description: ''\n        acceptance: ''\n",
        name = name,
        project = project_dir.display()
    )
}

fn snapshot_id_from_delete(body: &serde_json::Value) -> i64 {
    body["snapshotId"]
        .as_i64()
        .unwrap_or_else(|| panic!("snapshotId missing/wrong type in body: {body}"))
}

#[test]
fn list_snapshots_is_empty_on_a_fresh_dashboard() {
    let d = TestDashboard::new();
    let (s, body) = d.get("/api/snapshots");
    assert_eq!(s, 200, "list body: {body}");
    let arr = body["snapshots"]
        .as_array()
        .expect("snapshots array on response");
    assert!(arr.is_empty(), "fresh dashboard must have 0 snapshots");
}

#[test]
fn list_snapshots_returns_soft_deleted_plans_newest_first() {
    let d = TestDashboard::new();

    // Soft-delete two plans in sequence; the second should sort first.
    d.create_plan("alpha", &minimal_plan("alpha", &d.project));
    let (_, del_a) = d.delete("/api/plans/alpha");
    let id_a = snapshot_id_from_delete(&del_a);

    d.create_plan("beta", &minimal_plan("beta", &d.project));
    let (_, del_b) = d.delete("/api/plans/beta");
    let id_b = snapshot_id_from_delete(&del_b);

    let (s, body) = d.get("/api/snapshots");
    assert_eq!(s, 200, "list body: {body}");
    let snaps = body["snapshots"]
        .as_array()
        .expect("snapshots array on response");
    assert_eq!(snaps.len(), 2, "two soft deletes => two snapshots");

    // Newest first — ORDER BY created_at DESC, id DESC.
    assert_eq!(snaps[0]["id"].as_i64().unwrap(), id_b);
    assert_eq!(snaps[0]["planName"], "beta");
    assert_eq!(snaps[0]["kind"], "delete");
    assert!(
        snaps[0]["expiresAt"].as_str().is_some(),
        "expiresAt must be set: {body}"
    );
    assert!(
        snaps[0]["createdAt"].as_str().is_some(),
        "createdAt must be set: {body}"
    );
    assert!(
        snaps[0]["archivePath"].as_str().is_some(),
        "soft delete must record archivePath: {body}"
    );
    assert!(
        snaps[0]["restoredAt"].is_null(),
        "fresh snapshot's restoredAt must be NULL: {body}"
    );
    assert_eq!(snaps[1]["id"].as_i64().unwrap(), id_a);
    assert_eq!(snaps[1]["planName"], "alpha");
}

#[test]
fn list_snapshots_excludes_other_orgs() {
    let d = TestDashboard::new();
    d.create_plan("mine", &minimal_plan("mine", &d.project));
    let (_, del) = d.delete("/api/plans/mine");
    let snap_id = snapshot_id_from_delete(&del);

    // Reassign the snapshot to a foreign org by hand. The auth layer
    // resolves the caller to `default-org`, so this row should drop
    // out of the list.
    let db_path = d.dir.path().join(".claude").join("branchwork.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE plan_snapshots SET org_id = 'foreign-org' WHERE id = ?1",
            params![snap_id],
        )
        .unwrap();
    }

    let (s, body) = d.get("/api/snapshots");
    assert_eq!(s, 200, "list body: {body}");
    let arr = body["snapshots"].as_array().expect("snapshots array");
    assert!(
        arr.is_empty(),
        "foreign-org snapshot must not appear in default-org list: {body}"
    );
}

#[test]
fn delete_unknown_snapshot_returns_404() {
    let d = TestDashboard::new();
    let (s, body) = d.delete("/api/snapshots/424242");
    assert_eq!(s, 404, "body: {body}");
    assert_eq!(body["error"], "snapshot_not_found");
}

#[test]
fn delete_snapshot_removes_row_and_archive_and_audits() {
    let d = TestDashboard::new();
    d.create_plan("purge-me", &minimal_plan("purge-me", &d.project));
    let (_, del) = d.delete("/api/plans/purge-me");
    let snap_id = snapshot_id_from_delete(&del);
    let archive_path = del["archivePath"]
        .as_str()
        .expect("archivePath set on soft delete")
        .to_string();
    assert!(
        std::path::Path::new(&archive_path).exists(),
        "archive must exist after soft delete: {archive_path}"
    );

    let (s, body) = d.delete(&format!("/api/snapshots/{snap_id}"));
    assert_eq!(s, 200, "purge body: {body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["snapshotId"].as_i64().unwrap(), snap_id);
    assert_eq!(body["plan"], "purge-me");

    // Archive YAML on disk is gone.
    assert!(
        !std::path::Path::new(&archive_path).exists(),
        "archive must be removed: {archive_path}"
    );

    // DB row is gone.
    let db_path = d.dir.path().join(".claude").join("branchwork.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM plan_snapshots WHERE id = ?1",
            params![snap_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "snapshot row must be deleted");

    // Audit row exists with the expected action.
    let (action, resource_id, diff): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT action, resource_id, diff FROM audit_logs \
             WHERE action = 'plan.snapshot_purged_manual' AND resource_id = ?1",
            params!["purge-me"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("plan.snapshot_purged_manual audit row must exist");
    assert_eq!(action, "plan.snapshot_purged_manual");
    assert_eq!(resource_id.as_deref(), Some("purge-me"));
    let diff_val: serde_json::Value =
        serde_json::from_str(&diff.expect("diff present")).expect("diff is JSON");
    assert_eq!(diff_val["snapshot_id"].as_i64().unwrap(), snap_id);
    assert_eq!(diff_val["kind"], "delete");
    assert_eq!(diff_val["archive_path"], archive_path);

    // Subsequent delete is 404 (idempotent — the row is gone).
    let (s, body) = d.delete(&format!("/api/snapshots/{snap_id}"));
    assert_eq!(s, 404, "second delete must 404: {body}");
}

#[test]
fn delete_snapshot_succeeds_even_when_archive_already_gone() {
    let d = TestDashboard::new();
    d.create_plan(
        "archive-vanished",
        &minimal_plan("archive-vanished", &d.project),
    );
    let (_, del) = d.delete("/api/plans/archive-vanished");
    let snap_id = snapshot_id_from_delete(&del);
    let archive_path = del["archivePath"].as_str().unwrap().to_string();

    // Operator cleaned the archive directory by hand; the DB row is
    // still around. Purge should succeed without complaining.
    std::fs::remove_file(&archive_path).unwrap();

    let (s, body) = d.delete(&format!("/api/snapshots/{snap_id}"));
    assert_eq!(s, 200, "missing archive must not block purge: {body}");
    assert_eq!(body["ok"], true);
    assert!(
        body.get("warning").is_none(),
        "NotFound on archive remove is not a warning: {body}"
    );
}

#[test]
fn delete_snapshot_in_foreign_org_returns_403() {
    let d = TestDashboard::new();
    d.create_plan("foreign", &minimal_plan("foreign", &d.project));
    let (_, del) = d.delete("/api/plans/foreign");
    let snap_id = snapshot_id_from_delete(&del);

    // Move the snapshot to a different org so the caller's
    // default-org cannot touch it.
    let db_path = d.dir.path().join(".claude").join("branchwork.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE plan_snapshots SET org_id = 'foreign-org' WHERE id = ?1",
            params![snap_id],
        )
        .unwrap();
    }

    let (s, body) = d.delete(&format!("/api/snapshots/{snap_id}"));
    assert_eq!(s, 403, "cross-org purge must be 403: {body}");
    assert_eq!(body["error"], "snapshot_not_in_org");

    // Row is still there.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM plan_snapshots WHERE id = ?1",
            params![snap_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "403 must leave the snapshot row intact");
}
