import { useEffect, useRef, useMemo } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";
import { useAgentStore, type AgentOutputLine } from "../stores/agent-store.js";
import { usePlanStore } from "../stores/plan-store.js";

export function AgentPanel() {
  const selectedAgentId = useAgentStore((s) => s.selectedAgentId);
  const agents = useAgentStore((s) => s.agents);
  const agentOutput = useAgentStore((s) => s.agentOutput);
  const fetchAgentOutput = useAgentStore((s) => s.fetchAgentOutput);
  const killAgent = useAgentStore((s) => s.killAgent);
  const selectAgent = useAgentStore((s) => s.selectAgent);
  const plans = usePlanStore((s) => s.plans);

  const agent = agents.find((a) => a.id === selectedAgentId);
  const planTitle = agent?.plan_name
    ? plans.find((p) => p.name === agent.plan_name)?.title
    : null;

  const isPty = agent?.mode === "pty";
  const isActive = agent?.status === "running" || agent?.status === "starting";

  if (!agent) return null;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-2 border-b border-gray-800 flex items-center justify-between flex-shrink-0">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span
              className={`w-2 h-2 rounded-full flex-shrink-0 ${
                isActive ? "bg-emerald-500 animate-pulse" : agent.status === "completed" ? "bg-emerald-500" : "bg-red-500"
              }`}
            />
            <span className="text-sm font-medium truncate">
              {agent.task_id ? `Task ${agent.task_id}` : agent.id.slice(0, 8)}
            </span>
            <span className="text-[10px] text-gray-600">{agent.status}</span>
            <span className="text-[10px] text-gray-700">{agent.mode}</span>
          </div>
          {agent.plan_name && (
            <div className="text-[10px] text-gray-500 truncate">
              {planTitle ?? agent.plan_name}
              <span className="text-gray-700 font-mono ml-1">{agent.plan_name}</span>
            </div>
          )}
        </div>
        <div className="flex gap-1 flex-shrink-0">
          {isActive && (
            <button
              onClick={() => killAgent(agent.id)}
              className="px-2 py-1 text-xs bg-red-900/50 text-red-400 hover:bg-red-900 rounded transition"
            >
              Kill
            </button>
          )}
          <button
            onClick={() => selectAgent(null)}
            className="px-2 py-1 text-xs text-gray-500 hover:text-gray-300 rounded transition"
          >
            Close
          </button>
        </div>
      </div>

      {/* Content: PTY terminal or stream-json text view */}
      {isPty ? (
        <PtyTerminal agentId={agent.id} />
      ) : (
        <StreamJsonView agentId={agent.id} isActive={isActive} />
      )}
    </div>
  );
}

// --- PTY Terminal (xterm.js) ---

function PtyTerminal({ agentId }: { agentId: string }) {
  const termRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!termRef.current) return;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', Menlo, monospace",
      theme: {
        background: "#0a0a0f",
        foreground: "#e4e4e7",
        cursor: "#818cf8",
        selectionBackground: "#818cf840",
        black: "#18181b",
        red: "#f87171",
        green: "#4ade80",
        yellow: "#facc15",
        blue: "#60a5fa",
        magenta: "#c084fc",
        cyan: "#22d3ee",
        white: "#e4e4e7",
        brightBlack: "#52525b",
        brightRed: "#fca5a5",
        brightGreen: "#86efac",
        brightYellow: "#fde047",
        brightBlue: "#93c5fd",
        brightMagenta: "#d8b4fe",
        brightCyan: "#67e8f9",
        brightWhite: "#fafafa",
      },
      scrollback: 10000,
    });

    const fitAddon = new FitAddon();
    const webLinksAddon = new WebLinksAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(webLinksAddon);
    term.open(termRef.current);

    // Connect WebSocket
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(
      `${protocol}//${window.location.host}/terminal?agent=${agentId}`
    );

    ws.onopen = () => {
      // Fit after WS opens so we can send correct size
      requestAnimationFrame(() => {
        fitAddon.fit();
        ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
      });
    };

    ws.onmessage = (ev) => {
      term.write(ev.data);
    };

    ws.onclose = () => {
      term.write("\r\n\x1b[90m--- session ended ---\x1b[0m\r\n");
    };

    // Forward terminal input to WebSocket
    term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(data);
      }
    });

    // Handle resize
    const resizeObserver = new ResizeObserver(() => {
      requestAnimationFrame(() => {
        fitAddon.fit();
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }));
        }
      });
    });
    resizeObserver.observe(termRef.current);

    return () => {
      resizeObserver.disconnect();
      ws.close();
      term.dispose();
    };
  }, [agentId]);

  return (
    <div
      ref={termRef}
      className="flex-1 min-h-0"
      style={{ padding: "4px", background: "#0a0a0f" }}
    />
  );
}

// --- Stream-JSON text view (for check agents) ---

function StreamJsonView({ agentId, isActive }: { agentId: string; isActive: boolean }) {
  const agentOutput = useAgentStore((s) => s.agentOutput);
  const fetchAgentOutput = useAgentStore((s) => s.fetchAgentOutput);
  const outputRef = useRef<HTMLDivElement>(null);

  const output = agentOutput[agentId] ?? [];

  useEffect(() => {
    fetchAgentOutput(agentId);
  }, [agentId]);

  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [output.length]);

  // Extract verdict
  const verdict = useMemo(() => {
    for (let i = output.length - 1; i >= 0; i--) {
      try {
        const d = JSON.parse(output[i].content);
        if (d.type === "result" && d.result) {
          const m = d.result.match(/\{\s*"status"\s*:\s*"[^"]+"/);
          if (m) {
            const v = JSON.parse(m[0] + (m[0].endsWith("}") ? "" : "}"));
            if (v.status) return v as { status: string; reason: string };
          }
        }
      } catch { /* skip */ }
    }
    return null;
  }, [output]);

  const verdictColors: Record<string, string> = {
    completed: "text-emerald-400",
    in_progress: "text-amber-400",
    pending: "text-gray-400",
  };

  function renderLine(line: AgentOutputLine) {
    try {
      const d = JSON.parse(line.content);
      if (d.type === "assistant" && d.message?.content) {
        const texts = d.message.content
          .filter((b: { type: string }) => b.type === "text")
          .map((b: { text: string }) => b.text)
          .filter(Boolean);
        const toolUses = d.message.content
          .filter((b: { type: string }) => b.type === "tool_use")
          .map((b: { name: string }) => b.name);
        return (
          <div key={line.id}>
            {toolUses.length > 0 && (
              <div className="text-[11px] text-blue-400/70 py-0.5">
                {toolUses.map((t: string) => `[${t}]`).join(" ")}
              </div>
            )}
            {texts.length > 0 && (
              <div className="text-xs text-gray-200 py-1 whitespace-pre-wrap">
                {texts.join("\n")}
              </div>
            )}
          </div>
        );
      }
      if (d.type === "result") {
        const dur = d.duration_ms ? `${(d.duration_ms / 1000).toFixed(1)}s` : "";
        return (
          <div key={line.id} className="text-[10px] text-gray-600 py-1 border-t border-gray-800 mt-1">
            Finished in {dur} ({d.num_turns ?? 0} turns)
          </div>
        );
      }
      // Skip noise
      if (["system", "rate_limit_event", "user"].includes(d.type)) return null;
    } catch { /* skip */ }
    if (line.message_type === "stderr") {
      const text = line.content.trim();
      if (text.startsWith("Warning:")) return null;
      return <div key={line.id} className="text-[10px] text-red-400 py-0.5">{text}</div>;
    }
    return null;
  }

  const rendered = output.map(renderLine).filter(Boolean);

  return (
    <div className="flex-1 overflow-auto flex flex-col min-h-0">
      {/* Verdict banner */}
      {verdict && !isActive && (
        <div className="mx-3 mt-3 p-3 rounded border bg-gray-800/50 border-gray-700 flex-shrink-0">
          <span className={`text-sm font-semibold ${verdictColors[verdict.status] ?? "text-gray-300"}`}>
            {verdict.status === "completed" ? "Done" : verdict.status === "in_progress" ? "In Progress" : "Pending"}
          </span>
          <p className="text-xs text-gray-400 mt-1">{verdict.reason}</p>
        </div>
      )}

      {/* Output */}
      <div ref={outputRef} className="flex-1 overflow-auto p-3 space-y-0.5">
        {rendered.length === 0 && isActive && (
          <p className="text-xs text-gray-600">Agent is working...</p>
        )}
        {rendered.length === 0 && !isActive && (
          <p className="text-xs text-gray-600">No output.</p>
        )}
        {rendered}
      </div>
    </div>
  );
}
