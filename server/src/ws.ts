import { WebSocketServer, WebSocket } from "ws";
import type { Server } from "node:http";
import type { IncomingMessage } from "node:http";
import { attachTerminal, resizeAgent } from "./agent-manager.js";

let wss: WebSocketServer;
let termWss: WebSocketServer;

export function initWs(server: Server) {
  // Dashboard events WebSocket
  wss = new WebSocketServer({ noServer: true });
  wss.on("connection", (socket) => {
    socket.send(JSON.stringify({ type: "connected", timestamp: new Date().toISOString() }));
  });

  // Terminal WebSocket — one per agent
  termWss = new WebSocketServer({ noServer: true });
  termWss.on("connection", (socket, req) => {
    const url = new URL(req.url!, `http://${req.headers.host}`);
    const agentId = url.searchParams.get("agent");
    if (!agentId) {
      socket.close(4000, "Missing agent query param");
      return;
    }

    const attached = attachTerminal(agentId, socket);
    if (!attached) {
      socket.close(4001, "Agent not found or not a PTY agent");
      return;
    }

    // Handle resize messages
    socket.on("message", (data) => {
      try {
        const msg = JSON.parse(data.toString());
        if (msg.type === "resize" && msg.cols && msg.rows) {
          resizeAgent(agentId, msg.cols, msg.rows);
        }
      } catch {
        // Regular input — handled by attachTerminal
      }
    });
  });

  // Route upgrade requests by path
  server.on("upgrade", (req: IncomingMessage, socket, head) => {
    const pathname = new URL(req.url!, `http://${req.headers.host}`).pathname;
    if (pathname === "/ws") {
      wss.handleUpgrade(req, socket, head, (ws) => wss.emit("connection", ws, req));
    } else if (pathname === "/terminal") {
      termWss.handleUpgrade(req, socket, head, (ws) => termWss.emit("connection", ws, req));
    } else {
      socket.destroy();
    }
  });
}

export function broadcast(type: string, data: unknown) {
  if (!wss) return;
  const msg = JSON.stringify({ type, data, timestamp: new Date().toISOString() });
  for (const client of wss.clients) {
    if (client.readyState === WebSocket.OPEN) {
      client.send(msg);
    }
  }
}
