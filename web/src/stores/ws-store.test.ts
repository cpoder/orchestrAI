import { afterEach, describe, expect, it, vi } from "vitest";
import { useAgentStore } from "./agent-store.js";
import {
  usePlanStore,
  type ParsedPlan,
  type PlanSummary,
} from "./plan-store.js";
import { handleWsMessage } from "./ws-store.js";

afterEach(() => {
  // Reset zustand stores so seeded state and spies don't leak between tests.
  usePlanStore.setState({
    autoModeRuntimes: {},
    toasts: [],
    plans: [],
    selectedPlan: null,
  });
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

  it(
    "soft plan_deleted drops the plan, clears selectedPlan, " +
      "and pushes an Undo toast",
    () => {
      const summary: PlanSummary = {
        name: "doomed",
        title: "Doomed",
        project: null,
        phaseCount: 1,
        taskCount: 1,
        doneCount: 0,
        createdAt: "2026-04-12T00:00:00Z",
        modifiedAt: "2026-04-12T00:00:00Z",
      };
      const selected: ParsedPlan = {
        name: "doomed",
        filePath: "doomed.yaml",
        title: "Doomed",
        context: "",
        project: null,
        createdAt: "2026-04-12T00:00:00Z",
        modifiedAt: "2026-04-12T00:00:00Z",
        phases: [],
      };
      usePlanStore.setState({ plans: [summary], selectedPlan: selected });

      handleWsMessage({
        type: "plan_deleted",
        data: { plan: "doomed", snapshot_id: "snap-123", hard: false },
      });

      const state = usePlanStore.getState();
      expect(state.plans.find((p) => p.name === "doomed")).toBeUndefined();
      // App.tsx routes back to ProjectDashboard when selectedPlan is null.
      expect(state.selectedPlan).toBeNull();
      expect(state.toasts).toHaveLength(1);
      expect(state.toasts[0]).toMatchObject({
        kind: "info",
        message: "Deleted plan doomed",
        action: { label: "Undo", snapshotId: "snap-123" },
      });
    },
  );

  it(
    "hard plan_deleted (no snapshot_id) pushes a toast without an Undo action",
    () => {
      const summary: PlanSummary = {
        name: "obsolete",
        title: "Obsolete",
        project: null,
        phaseCount: 0,
        taskCount: 0,
        doneCount: 0,
        createdAt: "2026-04-12T00:00:00Z",
        modifiedAt: "2026-04-12T00:00:00Z",
      };
      usePlanStore.setState({ plans: [summary], selectedPlan: null });

      handleWsMessage({
        type: "plan_deleted",
        data: { plan: "obsolete", snapshot_id: null, hard: true },
      });

      const state = usePlanStore.getState();
      expect(state.plans.find((p) => p.name === "obsolete")).toBeUndefined();
      expect(state.toasts).toHaveLength(1);
      expect(state.toasts[0].action).toBeUndefined();
      expect(state.toasts[0]).toMatchObject({
        kind: "info",
        message: "Deleted plan obsolete",
      });
    },
  );

  it(
    "plan_deleted leaves selectedPlan alone when the user is viewing a different plan",
    () => {
      const other: ParsedPlan = {
        name: "still-here",
        filePath: "still-here.yaml",
        title: "Still here",
        context: "",
        project: null,
        createdAt: "2026-04-12T00:00:00Z",
        modifiedAt: "2026-04-12T00:00:00Z",
        phases: [],
      };
      usePlanStore.setState({ plans: [], selectedPlan: other });

      handleWsMessage({
        type: "plan_deleted",
        data: { plan: "doomed", snapshot_id: "snap-1", hard: false },
      });

      expect(usePlanStore.getState().selectedPlan).toEqual(other);
    },
  );
});
