import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import * as pty from "node-pty";
import { WebSocket } from "ws";
import { getDb } from "./db.js";
import { broadcast } from "./ws.js";

interface ManagedAgent {
  id: string;
  sessionId: string;
  planName?: string;
  taskId?: string;
  mode: "pty" | "stream-json";
  // PTY mode
  pty?: pty.IPty;
  terminals: Set<WebSocket>;
  // Stream-json mode
  process?: ChildProcess;
}

const agents = new Map<string, ManagedAgent>();

export function getActiveAgents() {
  const db = getDb();
  return db
    .prepare(`SELECT * FROM agents WHERE status IN ('starting', 'running') ORDER BY started_at DESC`)
    .all();
}

export function getAllAgents() {
  const db = getDb();
  return db.prepare(`SELECT * FROM agents ORDER BY started_at DESC LIMIT 50`).all();
}

export function getAgentOutput(agentId: string, limit = 200, offset = 0) {
  const db = getDb();
  return db
    .prepare(
      `SELECT * FROM agent_output WHERE agent_id = ? ORDER BY id ASC LIMIT ? OFFSET ?`
    )
    .all(agentId, limit, offset);
}

// Attach a WebSocket to receive PTY output and send input
export function attachTerminal(agentId: string, ws: WebSocket): boolean {
  const agent = agents.get(agentId);
  if (!agent) return false;

  agent.terminals.add(ws);

  // Send buffered output (replay)
  const db = getDb();
  const rows = db
    .prepare(`SELECT content FROM agent_output WHERE agent_id = ? AND message_type = 'pty' ORDER BY id`)
    .all(agentId) as { content: string }[];
  for (const row of rows) {
    ws.send(row.content);
  }

  // Forward user input from WebSocket to PTY (skip control messages)
  ws.on("message", (data) => {
    const msg = data.toString();
    // Skip JSON control messages (resize, etc.)
    if (msg.startsWith("{") && msg.includes('"type"')) return;
    if (agent.mode === "pty" && agent.pty) {
      agent.pty.write(msg);
    }
  });

  ws.on("close", () => {
    agent.terminals.delete(ws);
  });

  return true;
}

export function startAgent(opts: {
  prompt: string;
  cwd: string;
  planName?: string;
  taskId?: string;
  parentAgentId?: string;
  readOnly?: boolean;
  effort?: "low" | "medium" | "high" | "max";
}): string {
  if (opts.readOnly) {
    return startStreamJsonAgent(opts);
  }
  return startPtyAgent(opts);
}

// --- PTY mode: real interactive terminal ---

function startPtyAgent(opts: {
  prompt: string;
  cwd: string;
  planName?: string;
  taskId?: string;
  parentAgentId?: string;
}): string {
  const id = randomUUID();
  const sessionId = randomUUID();
  const db = getDb();

  db.prepare(
    `INSERT INTO agents (id, session_id, cwd, status, mode, plan_name, task_id, parent_agent_id, prompt)
     VALUES (?, ?, ?, 'starting', 'pty', ?, ?, ?, ?)`
  ).run(id, sessionId, opts.cwd, opts.planName ?? null, opts.taskId ?? null, opts.parentAgentId ?? null, opts.prompt);

  // Spawn claude in a real PTY — interactive mode with initial prompt
  const ptyArgs = [
    "--session-id", sessionId,
    "--add-dir", opts.cwd,
    "--verbose",
  ];
  if (opts.effort) ptyArgs.push("--effort", opts.effort);

  const shell = pty.spawn("claude", ptyArgs, {
    name: "xterm-256color",
    cols: 120,
    rows: 40,
    cwd: opts.cwd,
    env: { ...process.env, TERM: "xterm-256color" },
  });

  const agent: ManagedAgent = {
    id,
    sessionId,
    planName: opts.planName,
    taskId: opts.taskId,
    mode: "pty",
    pty: shell,
    terminals: new Set(),
  };
  agents.set(id, agent);

  db.prepare(`UPDATE agents SET pid = ?, status = 'running' WHERE id = ?`).run(
    shell.pid,
    id
  );

  broadcast("agent_started", { id, sessionId, planName: opts.planName, taskId: opts.taskId, pid: shell.pid, mode: "pty" });

  // Buffer PTY output and forward to connected terminals
  shell.onData((data) => {
    // Store in DB for replay
    db.prepare(
      `INSERT INTO agent_output (agent_id, message_type, content) VALUES (?, ?, ?)`
    ).run(id, "pty", data);

    // Forward to all connected WebSocket terminals
    for (const ws of agent.terminals) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(data);
      }
    }
  });

  shell.onExit(({ exitCode }) => {
    const status = exitCode === 0 ? "completed" : "failed";
    db.prepare(
      `UPDATE agents SET status = ?, finished_at = datetime('now') WHERE id = ?`
    ).run(status, id);
    agents.delete(id);

    // Close all terminal connections
    for (const ws of agent.terminals) {
      ws.close();
    }

    broadcast("agent_stopped", { id, status, exit_code: exitCode });
  });

  // Send the initial prompt once claude is ready (shows the input prompt "❯")
  let promptSent = false;
  shell.onData((data) => {
    if (promptSent) return;
    if (data.includes("❯") || data.includes("\u276f")) {
      promptSent = true;
      console.log(`[agent ${id.slice(0, 8)}] Ready signal detected, sending prompt (${opts.prompt.length} chars)`);
      setTimeout(() => {
        if (agent.pty) {
          agent.pty.write(opts.prompt + "\r");
          // Claude Code shows a paste confirmation for multi-line input — send a second CR to confirm
          setTimeout(() => {
            if (agent.pty) agent.pty.write("\r");
          }, 1000);
        }
      }, 500);
    }
  });
  // Fallback: if no prompt detected after 8s, send anyway
  setTimeout(() => {
    if (!promptSent && agent.pty) {
      promptSent = true;
      console.log(`[agent ${id.slice(0, 8)}] Fallback: sending prompt without ready signal`);
      agent.pty.write(opts.prompt + "\r");
      setTimeout(() => {
        if (agent.pty) agent.pty.write("\r");
      }, 1000);
    }
  }, 8000);

  return id;
}

// --- Stream-JSON mode: for read-only check agents ---

function startStreamJsonAgent(opts: {
  prompt: string;
  cwd: string;
  planName?: string;
  taskId?: string;
  parentAgentId?: string;
}): string {
  const id = randomUUID();
  const sessionId = randomUUID();
  const db = getDb();

  db.prepare(
    `INSERT INTO agents (id, session_id, cwd, status, mode, plan_name, task_id, parent_agent_id, prompt)
     VALUES (?, ?, ?, 'starting', 'stream-json', ?, ?, ?, ?)`
  ).run(id, sessionId, opts.cwd, opts.planName ?? null, opts.taskId ?? null, opts.parentAgentId ?? null, opts.prompt);

  const args = [
    "-p",
    "--verbose",
    "--output-format", "stream-json",
    "--input-format", "stream-json",
    "--session-id", sessionId,
    "--add-dir", opts.cwd,
    "--permission-mode", "plan",
    "--allowedTools", "Read,Glob,Grep,Bash(git:*)",
  ];
  if (opts.effort) args.push("--effort", opts.effort);

  const child = spawn("claude", args, {
    cwd: opts.cwd,
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env },
  });

  const initMsg = JSON.stringify({
    type: "user",
    message: { role: "user", content: [{ type: "text", text: opts.prompt }] },
  });
  child.stdin!.write(initMsg + "\n");
  child.stdin!.end();

  const agent: ManagedAgent = {
    id,
    sessionId,
    planName: opts.planName,
    taskId: opts.taskId,
    mode: "stream-json",
    process: child,
    terminals: new Set(),
  };
  agents.set(id, agent);

  db.prepare(`UPDATE agents SET pid = ?, status = 'running' WHERE id = ?`).run(child.pid, id);

  broadcast("agent_started", { id, sessionId, planName: opts.planName, taskId: opts.taskId, pid: child.pid, mode: "stream-json" });

  const rl = createInterface({ input: child.stdout! });
  rl.on("line", (line) => {
    try {
      const parsed = JSON.parse(line);
      db.prepare(
        `INSERT INTO agent_output (agent_id, message_type, content) VALUES (?, ?, ?)`
      ).run(id, parsed.type ?? "unknown", line);
      broadcast("agent_output", { agent_id: id, message_type: parsed.type, content: parsed });
    } catch {
      db.prepare(
        `INSERT INTO agent_output (agent_id, message_type, content) VALUES (?, ?, ?)`
      ).run(id, "raw", line);
    }
  });

  const stderrRl = createInterface({ input: child.stderr! });
  stderrRl.on("line", (line) => {
    db.prepare(
      `INSERT INTO agent_output (agent_id, message_type, content) VALUES (?, ?, ?)`
    ).run(id, "stderr", line);
  });

  child.on("exit", (code) => {
    const agentStatus = code === 0 ? "completed" : "failed";
    db.prepare(
      `UPDATE agents SET status = ?, finished_at = datetime('now') WHERE id = ?`
    ).run(agentStatus, id);
    agents.delete(id);

    // Parse verdict for check agents
    if (opts.taskId) {
      try {
        const outputRows = db
          .prepare(`SELECT content FROM agent_output WHERE agent_id = ? ORDER BY id`)
          .all(id) as { content: string }[];

        let verdictFound = false;
        for (const row of outputRows.reverse()) {
          try {
            const outer = JSON.parse(row.content);
            let text = "";
            if (outer.result) text = outer.result;
            else if (outer.message?.content) {
              for (const block of outer.message.content) {
                if (block.type === "text") text += block.text;
              }
            }
            const jsonMatch = text.match(/\{\s*"status"\s*:\s*"(completed|in_progress|pending)"[^}]*\}/);
            if (jsonMatch) {
              const verdict = JSON.parse(jsonMatch[0]);
              db.prepare(
                `INSERT INTO task_status (plan_name, task_number, status, updated_at)
                 VALUES (?, ?, ?, datetime('now'))
                 ON CONFLICT(plan_name, task_number)
                 DO UPDATE SET status = excluded.status, updated_at = datetime('now')`
              ).run(opts.planName, opts.taskId, verdict.status);
              broadcast("task_checked", {
                plan_name: opts.planName,
                task_number: opts.taskId,
                status: verdict.status,
                reason: verdict.reason ?? "",
                agent_id: id,
              });
              verdictFound = true;
            }
            if (verdictFound) break;
          } catch { /* skip */ }
        }
      } catch (e) {
        console.error(`[agent-manager] Failed to parse check result for agent ${id}:`, e);
      }
    }

    broadcast("agent_stopped", { id, status: agentStatus, exit_code: code });
  });

  return id;
}

export function sendMessageToAgent(agentId: string, message: string): boolean {
  const agent = agents.get(agentId);
  if (!agent) return false;
  if (agent.mode === "pty" && agent.pty) {
    agent.pty.write(message);
    return true;
  }
  return false;
}

export function resizeAgent(agentId: string, cols: number, rows: number) {
  const agent = agents.get(agentId);
  if (agent?.mode === "pty" && agent.pty) {
    agent.pty.resize(cols, rows);
  }
}

export function killAgent(agentId: string): boolean {
  const agent = agents.get(agentId);
  if (!agent) return false;

  if (agent.mode === "pty" && agent.pty) {
    agent.pty.kill();
  } else if (agent.process) {
    agent.process.kill("SIGTERM");
  }

  const db = getDb();
  db.prepare(
    `UPDATE agents SET status = 'killed', finished_at = datetime('now') WHERE id = ?`
  ).run(agentId);
  agents.delete(agentId);
  broadcast("agent_stopped", { id: agentId, status: "killed" });
  return true;
}
