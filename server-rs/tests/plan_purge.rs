//! E2E tests for the `plan_archive_retention_days` setting
//! (plan-deletion 0.5). The hourly purger itself is unit-tested in
//! `plan_curate::tests` — the integration tests here pin the HTTP
//! surface: settings round-trip, range validation, and the
//! retention-→-`expires_at` plumbing through `DELETE /api/plans/:name`.

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

#[test]
fn put_retention_round_trips_through_get_and_persists_to_disk() {
    let d = TestDashboard::new();

    // Default exposes 30 (DEFAULT_RETENTION_DAYS).
    let (s, body) = d.get("/api/settings");
    assert_eq!(s, 200, "settings GET: {body}");
    assert_eq!(
        body["plan_archive_retention_days"].as_i64(),
        Some(30),
        "default retention must be DEFAULT_RETENTION_DAYS"
    );

    // PUT a custom value.
    let (s, body) = d.put(
        "/api/settings",
        serde_json::json!({ "plan_archive_retention_days": 7 }),
    );
    assert_eq!(s, 200, "PUT body: {body}");
    assert_eq!(body["plan_archive_retention_days"].as_i64(), Some(7));

    // GET reflects it.
    let (_, body) = d.get("/api/settings");
    assert_eq!(body["plan_archive_retention_days"].as_i64(), Some(7));

    // The on-disk JSON also has it (the snapshot helper reads from
    // there directly, so this is the load-bearing assertion).
    let path = d
        .dir
        .path()
        .join(".claude")
        .join("branchwork-settings.json");
    let raw = std::fs::read_to_string(&path).expect("settings file written");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["plan_archive_retention_days"].as_i64(), Some(7));
}

#[test]
fn put_retention_rejects_out_of_range_values() {
    let d = TestDashboard::new();

    for bad in [-1i64, 366, 9999] {
        let (s, body) = d.put(
            "/api/settings",
            serde_json::json!({ "plan_archive_retention_days": bad }),
        );
        assert_eq!(s, 400, "value {bad} must 400, body: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("plan_archive_retention_days"),
            "error must mention the field, body: {body}"
        );
    }

    // Successful PUT after the rejected ones still works (state was
    // not corrupted by the failed attempts).
    let (s, _) = d.put(
        "/api/settings",
        serde_json::json!({ "plan_archive_retention_days": 14 }),
    );
    assert_eq!(s, 200);
}

#[test]
fn put_retention_accepts_min_and_max_boundary_values() {
    let d = TestDashboard::new();

    for ok in [0, 1, 365] {
        let (s, body) = d.put(
            "/api/settings",
            serde_json::json!({ "plan_archive_retention_days": ok }),
        );
        assert_eq!(s, 200, "value {ok} must succeed, body: {body}");
        assert_eq!(body["plan_archive_retention_days"].as_i64(), Some(ok));
    }
}

#[test]
fn retention_zero_delete_produces_immediately_purgeable_snapshot() {
    let d = TestDashboard::new();

    // Flip retention to 0 so the next soft delete writes a snapshot
    // whose expires_at is the same instant as created_at.
    let (s, _) = d.put(
        "/api/settings",
        serde_json::json!({ "plan_archive_retention_days": 0 }),
    );
    assert_eq!(s, 200);

    let plan = "expire-now";
    d.create_plan(plan, &minimal_plan(plan, &d.project));

    let (s, body) = d.delete(&format!("/api/plans/{plan}"));
    assert_eq!(s, 200, "delete body: {body}");
    let snapshot_id = body["snapshotId"]
        .as_i64()
        .expect("snapshotId set on soft delete");

    // The snapshot row exists and `expires_at <= datetime('now')` —
    // i.e. the next purge tick (`expires_at <= now() AND restored_at
    // IS NULL` selector in `plan_curate::select_expired`) would
    // remove it.
    let db_path = d.dir.path().join(".claude").join("branchwork.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let already_expired: i64 = conn
        .query_row(
            "SELECT CASE WHEN expires_at <= datetime('now') THEN 1 ELSE 0 END \
             FROM plan_snapshots WHERE id = ?1",
            params![snapshot_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        already_expired, 1,
        "retention=0 must produce a snapshot whose expires_at is already past"
    );
}

#[test]
fn default_retention_delete_produces_future_dated_snapshot() {
    let d = TestDashboard::new();
    let plan = "future-expiring";
    d.create_plan(plan, &minimal_plan(plan, &d.project));

    // Default retention (30 days) must NOT produce an expired snapshot.
    let (_, body) = d.delete(&format!("/api/plans/{plan}"));
    let snapshot_id = body["snapshotId"]
        .as_i64()
        .expect("snapshotId set on soft delete");

    let db_path = d.dir.path().join(".claude").join("branchwork.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let in_future: i64 = conn
        .query_row(
            "SELECT CASE WHEN expires_at > datetime('now') THEN 1 ELSE 0 END \
             FROM plan_snapshots WHERE id = ?1",
            params![snapshot_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        in_future, 1,
        "default retention must produce a snapshot expires_at in the future"
    );
}
