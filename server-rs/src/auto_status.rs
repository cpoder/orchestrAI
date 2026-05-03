use std::path::Path;
use std::process::Command;

/// Check if a file path (possibly relative, possibly just a filename) exists in the project.
#[allow(dead_code)] // unused while infer_status is disabled (2026-05-03); kept for the real replacement
pub fn find_file_in_project(project_dir: &Path, file_path: &str) -> bool {
    // Strip line number suffixes like :609-664 or :42
    let clean = file_path.split(':').next().unwrap_or(file_path).trim();

    // Direct path check
    let direct = project_dir.join(clean);
    if direct.exists() {
        return true;
    }

    // If it's just a filename (no directory separator) or has "...", search for it
    if !clean.contains('/') || clean.contains("...") {
        let filename = Path::new(clean)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(clean);

        if let Ok(output) = Command::new("find")
            .arg(project_dir)
            .arg("-name")
            .arg(filename)
            .arg("-not")
            .arg("-path")
            .arg("*/node_modules/*")
            .arg("-not")
            .arg("-path")
            .arg("*/.git/*")
            .arg("-not")
            .arg("-path")
            .arg("*/target/*")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return !stdout.trim().is_empty();
        }
    }

    false
}

/// Check git log for commits matching keywords.
#[allow(dead_code)] // Retained for future use — keyword grep disabled due to false positives
pub fn check_git_for_task(project_dir: &Path, keywords: &[&str]) -> usize {
    let git_dir = project_dir.join(".git");
    if !git_dir.exists() {
        return 0;
    }

    let mut hits = 0;
    for kw in keywords {
        if kw.len() < 4 {
            continue;
        }
        if let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(project_dir)
            .arg("log")
            .arg("--oneline")
            .arg("--all")
            .arg("-5")
            .arg(format!("--grep={kw}"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.trim().is_empty() {
                hits += 1;
            }
        }
    }
    hits
}

/// Determine task status based on file existence.
///
/// **Currently disabled — always returns `"pending"`.** The previous heuristic
/// flipped tasks to `"in_progress"` when any of their listed `file_paths`
/// existed on disk, but file existence cannot distinguish "an agent has
/// started this task" from "the file pre-existed because the task is meant to
/// modify it." For tasks that modify existing files (the majority of
/// branchwork's actual work) this produced false positives across an entire
/// plan on first sync — see the auto-mode-merge-ci-fix-loop incident on
/// 2026-05-03 where 17 of 20 tasks were silently flipped to in_progress.
///
/// A real replacement should derive `in_progress` from authoritative signals
/// (an `agents` row exists for the task and is not finished, or the task's
/// branch exists in the worktree), not from disk file existence. Until that
/// lands, this function returns `"pending"` for every input so the call
/// sites at `api/plans.rs` no longer corrupt state on sync.
pub fn infer_status(
    _project_dir: &Path,
    _file_paths: &[String],
    _title_words: &[&str],
) -> (&'static str, String) {
    (
        "pending",
        "auto-inference disabled (incident 2026-05-03)".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Disabled-mode contract: `infer_status` must return `"pending"` for
    /// every input, regardless of whether listed files exist on disk. This
    /// pins the stop-the-bleeding behaviour from the 2026-05-03 incident
    /// where the prior "files exist → in_progress" heuristic flipped 17/20
    /// pending tasks on a single plan sync. When the real
    /// agents/branch-aware replacement lands, this test gets replaced.
    #[test]
    fn infer_status_always_returns_pending() {
        let dir = tempdir().unwrap();
        for name in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            fs::write(dir.path().join(name), "").unwrap();
        }

        let cases: &[Vec<String>] = &[
            vec![],
            vec!["does/not/exist.rs".into(), "also/missing.ts".into()],
            vec!["a.rs".into(), "missing.rs".into()],
            vec!["a.rs".into(), "b.rs".into()],
            vec!["a.rs".into(), "b.rs".into(), "c.rs".into(), "d.rs".into()],
        ];

        for files in cases {
            let (status, _) = infer_status(dir.path(), files, &[]);
            assert_eq!(
                status, "pending",
                "infer_status returned non-pending for {files:?} — should be a no-op while disabled"
            );
        }
    }
}
