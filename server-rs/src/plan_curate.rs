//! Snapshot machinery for destructive plan operations.
//!
//! Every destructive plan primitive (delete, merge, rename, archive,
//! rewrite_context) writes a `plan_snapshots` row before mutating
//! state, so the action can be rolled back from the Activity tab.
//! The retention purger (plan-deletion 0.5) eventually frees expired
//! rows.
//!
//! This module is the shared home for snapshot machinery; the
//! merge / rename / archive primitives in
//! `project-plan-rearrange.yaml` will live alongside `snapshot_plan`
//! here.
//!
//! ## Cascade source
//!
//! [`snapshot_plan`] consults [`crate::api::plans::PLAN_CASCADE_TABLES`]
//! so any table classified by the 0.0 audit is automatically captured
//! — adding a new `plan_name`-keyed table only requires updating that
//! constant (and the cascade itself), not this module.

use std::path::Path;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::api::plans::PLAN_CASCADE_TABLES;
use crate::db::Db;
use crate::persisted_settings::PersistedSettings;
use crate::plan_parser;
use crate::state::AppState;

/// Default snapshot retention window in days when
/// `PersistedSettings.plan_archive_retention_days` is unset. The
/// admin-tab editor (plan-deletion 0.5) clamps to `0..=365`.
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// Why a snapshot was taken. Serialized as the `kind` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    Delete,
    Merge,
    Rename,
    Archive,
    RewriteContext,
}

impl SnapshotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Merge => "merge",
            Self::Rename => "rename",
            Self::Archive => "archive",
            Self::RewriteContext => "rewrite_context",
        }
    }
}

/// Errors returned by [`snapshot_plan`].
#[derive(Debug)]
pub enum SnapshotError {
    /// `plan_parser::find_plan_file` returned `None` for the slug.
    PlanNotFound(String),
    /// Reading the YAML body off disk failed.
    Io(std::io::Error),
    /// SQLite reported an error while reading or writing.
    Db(rusqlite::Error),
    /// `cascade_json` could not be serialized.
    Json(serde_json::Error),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlanNotFound(name) => write!(f, "plan {name} not found in plans_dir"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Db(e) => write!(f, "db: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for SnapshotError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}
impl From<serde_json::Error> for SnapshotError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Capture the full pre-cascade state of `plan_name` and write it
/// into `plan_snapshots`. Returns the new row id.
///
/// 1. Reads the plan YAML body off disk.
/// 2. For every table in [`PLAN_CASCADE_TABLES`], serializes every
///    row matching `plan_name = ?` as JSON into a single
///    `cascade_json` blob: `{ <table>: [<row>, ...], ... }`.
/// 3. Resolves `expires_at = now() + retention_days` from
///    `PersistedSettings.plan_archive_retention_days` (default
///    [`DEFAULT_RETENTION_DAYS`]). `retention_days = 0` produces
///    `expires_at == now()` so the next purge tick removes the
///    snapshot — useful for audit reconstruction even when the
///    user opted out of undo.
/// 4. Inherits `org_id` from the plan's `plan_org` row (or
///    `'default-org'` when unset).
/// 5. INSERTs into `plan_snapshots` and returns the new id.
///
/// The snapshot is durable across restarts; nothing is held in
/// memory. Callers MUST invoke this **before** running their
/// destructive cascade — a mid-cascade failure should still leave
/// the snapshot row available for manual recovery.
pub fn snapshot_plan(
    state: &AppState,
    plan_name: &str,
    kind: SnapshotKind,
    archive_path: Option<&Path>,
) -> Result<i64, SnapshotError> {
    let retention_days = read_retention_days(state);
    snapshot_plan_with_retention(
        &state.db,
        &state.plans_dir,
        plan_name,
        kind,
        archive_path,
        retention_days,
    )
}

/// Same as [`snapshot_plan`] but takes the cascade DB, plans_dir
/// and retention explicitly. Used by tests that want to drive a
/// specific retention window without round-tripping through a
/// real `AppState`.
pub(crate) fn snapshot_plan_with_retention(
    db: &Db,
    plans_dir: &Path,
    plan_name: &str,
    kind: SnapshotKind,
    archive_path: Option<&Path>,
    retention_days: i64,
) -> Result<i64, SnapshotError> {
    let plan_path = plan_parser::find_plan_file(plans_dir, plan_name)
        .ok_or_else(|| SnapshotError::PlanNotFound(plan_name.to_string()))?;
    let yaml_body = std::fs::read_to_string(&plan_path)?;

    let conn = db.lock().unwrap();
    let cascade_json = build_cascade_json(&conn, plan_name)?;
    let org_id: String = conn
        .query_row(
            "SELECT org_id FROM plan_org WHERE plan_name = ?1",
            params![plan_name],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "default-org".to_string());

    // SQLite's `datetime('now', '+N days')` accepts an integer N (and
    // 0 produces the same instant as `now()`), so retention=0 yields
    // `expires_at <= now()` exactly as the brief requires.
    conn.execute(
        "INSERT INTO plan_snapshots \
             (plan_name, kind, expires_at, org_id, archive_path, yaml_body, cascade_json) \
         VALUES (?1, ?2, datetime('now', ?3), ?4, ?5, ?6, ?7)",
        params![
            plan_name,
            kind.as_str(),
            format!("{retention_days:+} days"),
            org_id,
            archive_path.and_then(|p| p.to_str()),
            yaml_body,
            cascade_json,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn read_retention_days(state: &AppState) -> i64 {
    PersistedSettings::load(&state.settings_path)
        .plan_archive_retention_days
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

fn build_cascade_json(conn: &Connection, plan_name: &str) -> Result<String, SnapshotError> {
    let mut obj = serde_json::Map::with_capacity(PLAN_CASCADE_TABLES.len());
    for table in PLAN_CASCADE_TABLES {
        let rows = serialize_table_rows(conn, table, plan_name)?;
        obj.insert((*table).to_string(), serde_json::Value::Array(rows));
    }
    Ok(serde_json::to_string(&serde_json::Value::Object(obj))?)
}

fn serialize_table_rows(
    conn: &Connection,
    table: &str,
    plan_name: &str,
) -> Result<Vec<serde_json::Value>, SnapshotError> {
    // `SELECT *` so any future `ALTER TABLE ... ADD COLUMN` ships
    // into the snapshot automatically without touching this code.
    let sql = format!("SELECT * FROM {table} WHERE plan_name = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let rows = stmt
        .query_map(params![plan_name], |row| {
            let mut map = serde_json::Map::with_capacity(column_names.len());
            for (i, col) in column_names.iter().enumerate() {
                let value: rusqlite::types::Value = row.get(i)?;
                map.insert(col.clone(), value_to_json(value));
            }
            Ok(serde_json::Value::Object(map))
        })?
        .collect::<Result<Vec<_>, rusqlite::Error>>()?;
    Ok(rows)
}

fn value_to_json(v: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Real(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s),
        // No table in PLAN_CASCADE_TABLES uses BLOB today; render as a
        // length tag so the row round-trips and a 0.4 restore can flag
        // it explicitly rather than silently corrupting bytes.
        Value::Blob(b) => serde_json::Value::String(format!("__blob_{}_bytes__", b.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Set up a tempdir with a plans dir + a fresh migrated DB.
    /// Returns `(db, plans_dir, _tmp)` — the `_tmp` guard keeps the
    /// directory alive for the test duration.
    fn migrated_setup() -> (Db, PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let plans_dir = tmp.path().join("plans");
        std::fs::create_dir_all(&plans_dir).unwrap();
        let db_path = tmp.path().join("test.db");
        let db = crate::db::init(&db_path);
        (db, plans_dir, tmp)
    }

    fn write_plan_file(plans_dir: &Path, slug: &str, body: &str) -> PathBuf {
        let path = plans_dir.join(format!("{slug}.yaml"));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn seed_cascade_rows(db: &Db, plan: &str) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO task_status (plan_name, task_number, status) \
             VALUES (?1, '1.1', 'completed')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ci_runs (plan_name, task_number, status, run_url) \
             VALUES (?1, '1.1', 'success', 'https://example/runs/1')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_auto_mode (plan_name, enabled) VALUES (?1, 1)",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_auto_advance (plan_name, enabled) VALUES (?1, 1)",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_fix_attempts (plan_name, task_number, attempt, outcome) \
             VALUES (?1, '1.1', 1, 'success')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_project (plan_name, project) VALUES (?1, 'audit-proj')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_verdicts (plan_name, verdict, reason) VALUES (?1, 'ok', 'all green')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_budget (plan_name, max_budget_usd) VALUES (?1, 1.5)",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_learnings (plan_name, task_number, learning) \
             VALUES (?1, '1.1', 'noted')",
            params![plan],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO plan_org (plan_name, org_id) \
             VALUES (?1, 'default-org')",
            params![plan],
        )
        .unwrap();
    }

    fn read_snapshot_row(
        db: &Db,
        id: i64,
    ) -> (
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
    ) {
        let conn = db.lock().unwrap();
        conn.query_row(
            "SELECT plan_name, kind, created_at, expires_at, archive_path, yaml_body, cascade_json \
             FROM plan_snapshots WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn snapshot_plan_round_trips_cascade_rows() {
        let (db, plans_dir, _tmp) = migrated_setup();
        let plan = "round-trip-plan";
        let yaml = "title: round-trip\nphases: []\n";
        write_plan_file(&plans_dir, plan, yaml);
        seed_cascade_rows(&db, plan);

        let id = snapshot_plan_with_retention(
            &db,
            &plans_dir,
            plan,
            SnapshotKind::Delete,
            None,
            DEFAULT_RETENTION_DAYS,
        )
        .expect("snapshot_plan must succeed");
        assert!(id > 0);

        let (plan_name, kind, _created, _expires, archive_path, yaml_body, cascade_json) =
            read_snapshot_row(&db, id);
        assert_eq!(plan_name, plan);
        assert_eq!(kind, "delete");
        assert!(archive_path.is_none());
        assert_eq!(yaml_body, yaml);

        let parsed: serde_json::Value = serde_json::from_str(&cascade_json).unwrap();
        let obj = parsed.as_object().expect("cascade_json must be an object");
        // Every cascaded table appears as a key, even when its row
        // count is zero — gives the 0.4 restorer a stable shape.
        for table in PLAN_CASCADE_TABLES {
            assert!(
                obj.contains_key(*table),
                "cascade_json missing table {table}"
            );
        }
        // Each seeded row must round-trip: 1 row per cascaded table.
        for table in PLAN_CASCADE_TABLES {
            let rows = obj[*table].as_array().unwrap();
            assert_eq!(rows.len(), 1, "expected exactly one row in {table}");
            assert_eq!(rows[0]["plan_name"], serde_json::Value::String(plan.into()));
        }
        // Spot-check a value-bearing field on a couple of tables.
        assert_eq!(obj["plan_project"][0]["project"], "audit-proj");
        assert_eq!(obj["plan_budget"][0]["max_budget_usd"], 1.5);
        assert_eq!(obj["ci_runs"][0]["run_url"], "https://example/runs/1");
    }

    #[test]
    fn snapshot_plan_retention_zero_expires_immediately() {
        let (db, plans_dir, _tmp) = migrated_setup();
        let plan = "retention-zero";
        write_plan_file(&plans_dir, plan, "title: t\nphases: []\n");

        let id = snapshot_plan_with_retention(&db, &plans_dir, plan, SnapshotKind::Delete, None, 0)
            .unwrap();

        // expires_at <= datetime('now') — datetime() has 1-second
        // resolution so identical strings is the common case; the
        // <= cover the edge where the second flips between rows.
        let conn = db.lock().unwrap();
        let already_expired: i64 = conn
            .query_row(
                "SELECT CASE WHEN expires_at <= datetime('now') THEN 1 ELSE 0 END \
                 FROM plan_snapshots WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            already_expired, 1,
            "retention=0 must produce expires_at <= now()"
        );
    }

    #[test]
    fn snapshot_plan_kind_round_trips_each_variant() {
        let (db, plans_dir, _tmp) = migrated_setup();
        let plan = "kind-roundtrip";
        write_plan_file(&plans_dir, plan, "title: t\nphases: []\n");

        for (kind, expected) in [
            (SnapshotKind::Delete, "delete"),
            (SnapshotKind::Merge, "merge"),
            (SnapshotKind::Rename, "rename"),
            (SnapshotKind::Archive, "archive"),
            (SnapshotKind::RewriteContext, "rewrite_context"),
        ] {
            let id = snapshot_plan_with_retention(
                &db,
                &plans_dir,
                plan,
                kind,
                None,
                DEFAULT_RETENTION_DAYS,
            )
            .unwrap();
            let stored: String = db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT kind FROM plan_snapshots WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                stored, expected,
                "snapshot kind column must reflect caller's variant"
            );
        }
    }

    #[test]
    fn snapshot_plan_records_archive_path_when_provided() {
        let (db, plans_dir, _tmp) = migrated_setup();
        let plan = "with-archive";
        write_plan_file(&plans_dir, plan, "title: t\nphases: []\n");
        let archive = plans_dir
            .join("archive")
            .join("with-archive.20260512Z.yaml");

        let id = snapshot_plan_with_retention(
            &db,
            &plans_dir,
            plan,
            SnapshotKind::Delete,
            Some(&archive),
            DEFAULT_RETENTION_DAYS,
        )
        .unwrap();
        let (_, _, _, _, archive_path, _, _) = read_snapshot_row(&db, id);
        assert_eq!(archive_path.as_deref(), archive.to_str());
    }

    #[test]
    fn snapshot_plan_inherits_org_from_plan_org() {
        let (db, plans_dir, _tmp) = migrated_setup();
        let plan = "org-tagged";
        write_plan_file(&plans_dir, plan, "title: t\nphases: []\n");
        // plan_org references organizations(id) via FK; seed an org
        // first so the foreign key holds.
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO organizations (id, name, slug) \
                 VALUES ('org-curate', 'Curate Org', 'curate-org')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO plan_org (plan_name, org_id) VALUES (?1, 'org-curate')",
                params![plan],
            )
            .unwrap();
        }

        let id = snapshot_plan_with_retention(
            &db,
            &plans_dir,
            plan,
            SnapshotKind::Delete,
            None,
            DEFAULT_RETENTION_DAYS,
        )
        .unwrap();
        let stored_org: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT org_id FROM plan_snapshots WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_org, "org-curate");
    }

    #[test]
    fn snapshot_plan_returns_plan_not_found_when_yaml_missing() {
        let (db, plans_dir, _tmp) = migrated_setup();
        // No file written.
        let err = snapshot_plan_with_retention(
            &db,
            &plans_dir,
            "missing",
            SnapshotKind::Delete,
            None,
            DEFAULT_RETENTION_DAYS,
        )
        .unwrap_err();
        match err {
            SnapshotError::PlanNotFound(name) => assert_eq!(name, "missing"),
            other => panic!("expected PlanNotFound, got {other:?}"),
        }
    }
}
