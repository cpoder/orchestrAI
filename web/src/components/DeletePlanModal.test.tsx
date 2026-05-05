import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import axe from "axe-core";
import { DeletePlanModal } from "./DeletePlanModal.js";
import { usePlanStore } from "../stores/plan-store.js";
import { useAgentStore } from "../stores/agent-store.js";
import { HttpError } from "../api.js";

const PLAN = "scratch-plan";

beforeEach(() => {
  // Replace store actions with spies; tests assert on these.
  usePlanStore.setState({
    deletePlan: vi.fn().mockResolvedValue({
      ok: true,
      name: PLAN,
      snapshotId: 42,
      archivePath: "/p/archive/x.yaml",
      hard: false,
    }),
  });
  useAgentStore.setState({ selectAgent: vi.fn() });
});

afterEach(() => {
  cleanup();
});

describe("DeletePlanModal", () => {
  it("renders the soft-delete copy and shows the Shift hint when retention > 0", () => {
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={() => {}}
      />,
    );
    expect(
      screen.getByRole("heading", { name: /Delete plan scratch-plan\?/i }),
    ).toBeTruthy();
    expect(
      screen.getByText(/Recoverable for 30 days from the Activity tab/i),
    ).toBeTruthy();
    // Shift hint mentions Shift modifier verbatim.
    expect(screen.getByText(/Hold/)).toBeTruthy();
    expect(screen.getByText("Shift")).toBeTruthy();
  });

  it("uses singular 'day' when retention is exactly 1", () => {
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={1}
        onClose={() => {}}
      />,
    );
    expect(
      screen.getByText(/Recoverable for 1 day from the Activity tab/i),
    ).toBeTruthy();
  });

  it("opens directly in hard-confirm mode when retentionDays is 0", () => {
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={0}
        onClose={() => {}}
      />,
    );
    expect(
      screen.getByRole("heading", {
        name: /Permanently delete plan scratch-plan\?/i,
      }),
    ).toBeTruthy();
    expect(
      screen.getByText(/Permanently deletes the plan file/i),
    ).toBeTruthy();
    expect(screen.getByText(/This cannot be undone/i)).toBeTruthy();
    // No Shift hint — modifier is meaningless when retention is 0.
    expect(screen.queryByText(/Hold/)).toBeNull();
    // Primary button text is the permanent variant.
    expect(
      screen.getByRole("button", { name: /Permanently delete/i }),
    ).toBeTruthy();
  });

  it("Cancel does not call deletePlan and invokes onClose", () => {
    const onClose = vi.fn();
    const deletePlan = vi.fn();
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Cancel$/ }));
    expect(deletePlan).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("plain Delete click triggers a soft delete and closes the modal", async () => {
    const onClose = vi.fn();
    const deletePlan = vi.fn().mockResolvedValue({
      ok: true,
      name: PLAN,
      snapshotId: 7,
      archivePath: null,
      hard: false,
    });
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() => expect(deletePlan).toHaveBeenCalledTimes(1));
    // Soft delete: opts is undefined so the call site sends no `?hard=true`.
    expect(deletePlan).toHaveBeenCalledWith(PLAN, undefined);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("Shift+click flips to hard-confirm without calling deletePlan; second click commits hard", async () => {
    const onClose = vi.fn();
    const deletePlan = vi.fn().mockResolvedValue({
      ok: true,
      name: PLAN,
      snapshotId: null,
      archivePath: null,
      hard: true,
    });
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    // First click holds Shift — should NOT issue the delete.
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }), {
      shiftKey: true,
    });
    expect(deletePlan).not.toHaveBeenCalled();
    // Modal re-renders into hard-confirm mode.
    await waitFor(() =>
      expect(
        screen.getByRole("heading", {
          name: /Permanently delete plan scratch-plan\?/i,
        }),
      ).toBeTruthy(),
    );
    // Second click on the new "Permanently delete" button issues hard delete.
    fireEvent.click(
      screen.getByRole("button", { name: /Permanently delete/i }),
    );
    await waitFor(() => expect(deletePlan).toHaveBeenCalledTimes(1));
    expect(deletePlan).toHaveBeenCalledWith(PLAN, { hard: true });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("409 plan_has_running_agents keeps the modal open and lists agent IDs", async () => {
    const onClose = vi.fn();
    const deletePlan = vi.fn().mockRejectedValue(
      new HttpError(409, "Conflict", {
        error: "plan_has_running_agents",
        agents: ["agent-aaa", "agent-bbb"],
      }),
    );
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() =>
      expect(
        screen.getByText(/this plan has running agents/i),
      ).toBeTruthy(),
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "agent-aaa" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "agent-bbb" })).toBeTruthy();
  });

  it("clicking an agent link calls selectAgent and closes the modal", async () => {
    const onClose = vi.fn();
    const selectAgent = vi.fn();
    const deletePlan = vi.fn().mockRejectedValue(
      new HttpError(409, "Conflict", {
        error: "plan_has_running_agents",
        agents: ["agent-xyz"],
      }),
    );
    usePlanStore.setState({ deletePlan });
    useAgentStore.setState({ selectAgent });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "agent-xyz" })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: "agent-xyz" }));
    expect(selectAgent).toHaveBeenCalledWith("agent-xyz");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("409 auto_mode_in_flight surfaces an explanatory banner", async () => {
    const onClose = vi.fn();
    const deletePlan = vi.fn().mockRejectedValue(
      new HttpError(409, "Conflict", { error: "auto_mode_in_flight" }),
    );
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() =>
      expect(
        screen.getByText(/auto-mode is mid-flight/i),
      ).toBeTruthy(),
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it("404 (plan already gone) closes the modal silently", async () => {
    const onClose = vi.fn();
    const deletePlan = vi
      .fn()
      .mockRejectedValue(
        new HttpError(404, "Not Found", { error: "plan_not_found" }),
      );
    usePlanStore.setState({ deletePlan });
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("ESC closes the modal", () => {
    const onClose = vi.fn();
    render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={onClose}
      />,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("returns focus to the trigger element on unmount", () => {
    const trigger = document.createElement("button");
    trigger.id = "open-modal";
    document.body.appendChild(trigger);
    trigger.focus();
    const { unmount } = render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={() => {}}
      />,
    );
    // While the modal is mounted, focus should have moved off the trigger.
    expect(document.activeElement).not.toBe(trigger);
    unmount();
    expect(document.activeElement).toBe(trigger);
    document.body.removeChild(trigger);
  });

  it("axe-core reports zero violations on the rendered dialog", async () => {
    const { container } = render(
      <DeletePlanModal
        planName={PLAN}
        retentionDays={30}
        onClose={() => {}}
      />,
    );
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    const results = await axe.run(dialog as Element, {
      // jsdom doesn't compute layout / colors so visual rules
      // (color-contrast in particular) can't be checked here. We
      // restrict to structural a11y rules — focus management, ARIA
      // semantics, keyboard reachability — which is exactly what
      // the task brief lists (focus trap, return-focus,
      // role="dialog").
      runOnly: {
        type: "tag",
        values: ["wcag2a", "wcag2aa"],
      },
      rules: { "color-contrast": { enabled: false } },
    });
    expect(results.violations).toEqual([]);
  });
});
