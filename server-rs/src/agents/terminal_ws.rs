use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use serde::Deserialize;

use crate::agents::AgentRegistry;
use crate::agents::session_protocol::Message as SessionMessage;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct TerminalQuery {
    agent: String,
}

pub async fn terminal_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<TerminalQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_terminal(socket, query.agent, state.registry))
}

async fn handle_terminal(mut socket: WebSocket, agent_id: String, registry: AgentRegistry) {
    // Load buffered output from DB first. Users always want the historical
    // transcript, even when the live agent is still going.
    let buffered_rows: Vec<String> = {
        let db = registry.db.lock().unwrap();
        let mut stmt = db
            .prepare(
                "SELECT content FROM agent_output \
                 WHERE agent_id = ? AND message_type = 'pty' ORDER BY id",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![agent_id], |row| row.get(0))
            .unwrap()
            .flatten()
            .collect()
    };

    // Subscribe to the live output broadcast (if the agent is still live).
    // `command_tx` is what lets us write keystrokes + resize into the
    // daemon's PTY.
    let subscription = {
        let agents = registry.agents.lock().await;
        agents
            .get(&agent_id)
            .map(|agent| (agent.output_tx.subscribe(), agent.command_tx.clone()))
    };

    for row in &buffered_rows {
        if socket
            .send(Message::Text(row.clone().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    let (mut output_rx, command_tx) = match subscription {
        Some(s) => s,
        None => {
            // Not live in-process — figure out whether it's still running
            // (detached / server restarted mid-agent) or truly finished.
            let status = {
                let db = registry.db.lock().unwrap();
                db.query_row(
                    "SELECT status FROM agents WHERE id = ?",
                    rusqlite::params![agent_id],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            };
            let is_running = matches!(status.as_deref(), Some("running") | Some("starting"));
            if is_running {
                socket
                    .send(Message::Text(
                        "\r\n\x1b[33m--- terminal detached (server restarted while agent was running) ---\x1b[0m\r\n\
                         \x1b[33m--- agent is still alive — use Kill to stop it, or wait for it to finish ---\x1b[0m\r\n".into(),
                    ))
                    .await
                    .ok();
            } else {
                socket
                    .send(Message::Text(
                        "\r\n\x1b[90m--- session ended ---\x1b[0m\r\n".into(),
                    ))
                    .await
                    .ok();
            }
            return;
        }
    };

    // Live bidirectional proxy: supervisor → browser, browser → supervisor.
    loop {
        tokio::select! {
            // Live PTY output from the daemon → browser
            recv = output_rx.recv() => {
                match recv {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    // Broadcast buffer overflow: skip the lag and keep going.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            // Browser → supervisor. Text frames may carry resize JSON or
            // literal keystrokes; binary frames are always keystrokes.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if text.starts_with('{')
                            && let Ok(val) = serde_json::from_str::<serde_json::Value>(&text)
                            && val.get("type").and_then(|t| t.as_str()) == Some("resize")
                        {
                            let cols = val.get("cols").and_then(|c| c.as_u64()).unwrap_or(120) as u16;
                            let rows = val.get("rows").and_then(|r| r.as_u64()).unwrap_or(40) as u16;
                            let _ = command_tx.send(SessionMessage::Resize { cols, rows });
                            continue;
                        }
                        let _ = command_tx.send(SessionMessage::Input(text.as_bytes().to_vec()));
                    }
                    Some(Ok(Message::Binary(data))) => {
                        let _ = command_tx.send(SessionMessage::Input(data.to_vec()));
                    }
                    None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
