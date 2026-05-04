import { afterEach, describe, expect, it, vi } from "vitest";
import { useAgentStore } from "./agent-store.js";
import { usePlanStore } from "./plan-store.js";
import { handleWsMessage } from "./ws-store.js";

afterEach(() => {
  // Reset zustand stores so seeded state and spies don't leak between tests.
  usePlanStore.setState({ autoModeRuntimes: {} });
});

describe("ws-store handleWsMessage", () => {
  it("refreshes agents on task_advanced", () => {
    const fetchAgents = vi.fn().mockResolvedValue(undefined);
    useAgentStore.setState({ fetchAgents });

    handleWsMessage({
      type: "task_advanced",
      data: {
        plan: "fix-plan-done-in-progress",
        from_task: "1.1",
        to_tasks: ["1.2", "1.3"],
      },
    });

    expect(fetchAgents).toHaveBeenCalledTimes(1);
  });

  it("sets auto_finishing pill state on auto_finish_triggered", () => {
    const fetchAgents = vi.fn().mockResolvedValue(undefined);
    useAgentStore.setState({ fetchAgents });

    handleWsMessage({
      type: "auto_finish_triggered",
      data: {
        agent_id: "abc-123",
        plan: "unattended-auto-mode",
        task: "6.1",
        trigger: "stop_hook",
      },
    });

    const runtime = usePlanStore.getState()
      .autoModeRuntimes["unattended-auto-mode"];
    expect(runtime).toEqual({
      state: "auto_finishing",
      task: "6.1",
    });
    // Stop-hook path also defensively refreshes agents so the row's
    // stop_reason flips visibly before `agent_stopped` lands.
    expect(fetchAgents).toHaveBeenCalledTimes(1);
  });
});
