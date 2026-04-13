use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::process::Command;

use rusqlite::params;

use crate::agents::{AgentMode, AgentRegistry, ManagedAgent};
use crate::config::Effort;
use crate::ws::broadcast_event;

fn tmux_session_name(agent_id: &str) -> String {
    format!("oai-{}", &agent_id[..8])
}

fn pipe_path(agent_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("orchestrai-{}.pipe", &agent_id[..8]))
}

pub async fn start_pty_agent(
    registry: &AgentRegistry,
    prompt: String,
    cwd: &Path,
    plan_name: Option<&str>,
    task_id: Option<&str>,
    effort: Effort,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let tmux_name = tmux_session_name(&id);
    let pipe = pipe_path(&id);

    // Insert into DB
    {
        let db = registry.db.lock().unwrap();
        db.execute(
            "INSERT INTO agents (id, session_id, cwd, status, mode, plan_name, task_id, prompt)
             VALUES (?1, ?2, ?3, 'starting', 'pty', ?4, ?5, ?6)",
            params![id, session_id, cwd.to_str().unwrap_or(""), plan_name, task_id, prompt],
        ).ok();
    }

    // Create named pipe for output streaming
    if pipe.exists() {
        std::fs::remove_file(&pipe).ok();
    }
    unsafe { libc::mkfifo(std::ffi::CString::new(pipe.to_str().unwrap()).unwrap().as_ptr(), 0o600); }

    // Create tmux session running claude
    let claude_cmd = format!(
        "claude --session-id {} --add-dir {} --verbose --effort {}",
        session_id,
        shell_escape(cwd.to_str().unwrap_or(".")),
        effort,
    );

    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", &tmux_name, "-x", "120", "-y", "40", &claude_cmd])
        .current_dir(cwd)
        .env("TERM", "xterm-256color")
        .status();

    if status.is_err() || !status.unwrap().success() {
        eprintln!("[agent {}] Failed to create tmux session", &id[..8]);
        let db = registry.db.lock().unwrap();
        db.execute("UPDATE agents SET status = 'failed', finished_at = datetime('now') WHERE id = ?", params![id]).ok();
        return id;
    }

    // Get the PID of the claude process inside tmux
    let pid = get_tmux_pid(&tmux_name).unwrap_or(0);

    {
        let db = registry.db.lock().unwrap();
        db.execute("UPDATE agents SET pid = ?1, status = 'running' WHERE id = ?2", params![pid as i64, id]).ok();
    }

    broadcast_event(
        &registry.broadcast_tx,
        "agent_started",
        serde_json::json!({"id": id, "sessionId": session_id, "planName": plan_name, "taskId": task_id, "pid": pid, "mode": "pty"}),
    );

    // Start pipe-pane to stream tmux output to our named pipe
    Command::new("tmux")
        .args(["pipe-pane", "-t", &tmux_name, "-o", &format!("cat >> {}", pipe.display())])
        .status().ok();

    // Register agent
    let agent = ManagedAgent {
        id: id.clone(),
        session_id,
        plan_name: plan_name.map(|s| s.to_string()),
        task_id: task_id.map(|s| s.to_string()),
        mode: AgentMode::Pty,
        pty: None,
        pty_writer: None,
        pty_master: None,
        tmux_session: Some(tmux_name.clone()),
        terminals: Vec::new(),
    };
    registry.agents.lock().await.insert(id.clone(), agent);

    // Spawn reader thread: reads from named pipe, stores in DB, forwards to terminals
    spawn_pipe_reader(registry, &id, &pipe);

    // Send initial prompt once claude is ready
    let prompt_sent = Arc::new(AtomicBool::new(false));
    let tmux_for_prompt = tmux_name.clone();
    let prompt_for_send = prompt.clone();
    let prompt_sent_clone = prompt_sent.clone();

    // Watch for ready signal in the output
    let db_watch = registry.db.clone();
    let id_watch = id.clone();
    let tmux_watch = tmux_name.clone();
    let prompt_watch = prompt;
    let prompt_sent_watch = prompt_sent.clone();
    tokio::spawn(async move {
        // Poll tmux pane content for the ready signal
        for _ in 0..80 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            if prompt_sent_watch.load(Ordering::Relaxed) { return; }

            if let Ok(output) = Command::new("tmux")
                .args(["capture-pane", "-t", &tmux_watch, "-p"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                if text.contains('❯') || text.contains('\u{276f}') {
                    if prompt_sent_watch.swap(true, Ordering::Relaxed) { return; }
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    send_to_tmux(&tmux_watch, &prompt_watch);
                    // Second enter for paste confirmation
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    send_to_tmux_raw(&tmux_watch, "Enter");
                    return;
                }
            }
        }
        // Fallback: 16s timeout, send anyway
        if !prompt_sent_watch.swap(true, Ordering::Relaxed) {
            send_to_tmux(&tmux_watch, &prompt_watch);
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            send_to_tmux_raw(&tmux_watch, "Enter");
        }
    });

    id
}

/// Reattach to an existing tmux session (for agents that survived a server restart)
pub async fn reattach_agent(registry: &AgentRegistry, agent_id: &str, tmux_name: &str) {
    let pipe = pipe_path(agent_id);

    // Recreate pipe if missing
    if !pipe.exists() {
        unsafe { libc::mkfifo(std::ffi::CString::new(pipe.to_str().unwrap()).unwrap().as_ptr(), 0o600); }
        // Restart pipe-pane
        Command::new("tmux")
            .args(["pipe-pane", "-t", tmux_name, "-o", &format!("cat >> {}", pipe.display())])
            .status().ok();
    }

    let agent = ManagedAgent {
        id: agent_id.to_string(),
        session_id: String::new(),
        plan_name: None,
        task_id: None,
        mode: AgentMode::Pty,
        pty: None,
        pty_writer: None,
        pty_master: None,
        tmux_session: Some(tmux_name.to_string()),
        terminals: Vec::new(),
    };
    registry.agents.lock().await.insert(agent_id.to_string(), agent);

    spawn_pipe_reader(registry, agent_id, &pipe);
    println!("[orchestrAI] Reattached agent {} to tmux session {}", &agent_id[..8], tmux_name);
}

fn spawn_pipe_reader(registry: &AgentRegistry, agent_id: &str, pipe: &Path) {
    let db = registry.db.clone();
    let agents = registry.agents.clone();
    let tx = registry.broadcast_tx.clone();
    let id = agent_id.to_string();
    let pipe = pipe.to_path_buf();

    thread::spawn(move || {
        // Open pipe (blocks until writer connects — tmux pipe-pane)
        let file = match std::fs::File::open(&pipe) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[agent {}] Failed to open pipe: {e}", &id[..8]);
                return;
            }
        };
        let reader = std::io::BufReader::new(file);
        let mut buf = [0u8; 4096];

        // Read chunks
        use std::io::Read;
        let mut reader = reader;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // Pipe closed — tmux session ended
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];
                    let text = String::from_utf8_lossy(data).to_string();

                    // Store in DB
                    {
                        let db = db.lock().unwrap();
                        db.execute(
                            "INSERT INTO agent_output (agent_id, message_type, content) VALUES (?1, 'pty', ?2)",
                            params![id, text],
                        ).ok();
                    }

                    // Forward to terminals
                    let agents_guard = agents.blocking_lock();
                    if let Some(agent) = agents_guard.get(&id) {
                        for tx in &agent.terminals {
                            tx.send(data.to_vec()).ok();
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Agent exited — update DB
        {
            let db = db.lock().unwrap();
            db.execute(
                "UPDATE agents SET status = 'completed', finished_at = datetime('now') WHERE id = ? AND status = 'running'",
                params![id],
            ).ok();
        }
        agents.blocking_lock().remove(&id);
        broadcast_event(&tx, "agent_stopped", serde_json::json!({"id": id, "status": "completed", "exit_code": 0}));

        // Cleanup pipe
        std::fs::remove_file(&pipe).ok();
    });
}

fn send_to_tmux(session: &str, text: &str) {
    // Use send-keys with literal flag to avoid key interpretation
    Command::new("tmux")
        .args(["send-keys", "-t", session, "-l", text])
        .status().ok();
    // Send Enter
    Command::new("tmux")
        .args(["send-keys", "-t", session, "Enter"])
        .status().ok();
}

fn send_to_tmux_raw(session: &str, key: &str) {
    Command::new("tmux")
        .args(["send-keys", "-t", session, key])
        .status().ok();
}

fn get_tmux_pid(session: &str) -> Option<u32> {
    let output = Command::new("tmux")
        .args(["list-panes", "-t", session, "-F", "#{pane_pid}"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse().ok()
}

fn shell_escape(s: &str) -> String {
    if s.contains(' ') || s.contains('\'') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}
