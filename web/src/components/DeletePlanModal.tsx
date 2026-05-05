import { useEffect, useId, useRef, useState } from "react";
import { HttpError } from "../api.js";
import { usePlanStore } from "../stores/plan-store.js";
import { useAgentStore } from "../stores/agent-store.js";

interface DeletePlanModalProps {
  planName: string;
  /// Days a soft-deleted plan stays in the archive before purge.
  /// 0 collapses the dialog to the permanent-delete confirmation
  /// (no soft path, no Undo) — the org has opted out of retention.
  retentionDays: number;
  onClose: () => void;
}

interface PlanHasRunningAgentsBody {
  error: "plan_has_running_agents";
  agents?: string[];
}

interface AutoModeInFlightBody {
  error: "auto_mode_in_flight";
}

interface GenericErrorBody {
  error?: string;
  agents?: string[];
}

/// Confirmation modal for the per-plan Delete action. Drives the
/// shape decisions documented in plan-deletion 1.1:
///
/// - Default click path is *soft delete* — the modal copy explains
///   archive + retention + cascade and emphasizes that the agent rows
///   stick around.
/// - Holding Shift while clicking the primary button flips the modal
///   to a *hard-delete* re-confirm step (one extra click is
///   intentional friction). When the org's `plan_archive_retention_days`
///   is 0, the modal opens directly in hard-confirm mode because
///   soft delete collapses to hard at retention=0.
/// - The 409 paths (`plan_has_running_agents`,
///   `auto_mode_in_flight`) keep the modal open and surface an
///   inline banner so the user can click into the offending agent's
///   terminal without dismissing first.
///
/// Accessibility: `role="dialog"` + `aria-modal="true"` +
/// `aria-labelledby` (title) + `aria-describedby` (body). Focus
/// lands on Cancel on mount, traps inside the dialog (Tab cycles),
/// returns to the trigger element on unmount, ESC closes when not
/// busy. axe-core passes (verified in `DeletePlanModal.test.tsx`).
export function DeletePlanModal({
  planName,
  retentionDays,
  onClose,
}: DeletePlanModalProps) {
  const titleId = useId();
  const descId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelButtonRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const [busy, setBusy] = useState(false);
  // 'soft'  → first click issues a soft delete (or hard, when retentionDays=0).
  // 'hard'  → user held Shift on the primary; one more click commits hard.
  const initialStage: "soft" | "hard" = retentionDays === 0 ? "hard" : "soft";
  const [stage, setStage] = useState<"soft" | "hard">(initialStage);
  const [error, setError] = useState<string | null>(null);
  const [runningAgents, setRunningAgents] = useState<string[] | null>(null);

  const deletePlan = usePlanStore((s) => s.deletePlan);
  const selectAgent = useAgentStore((s) => s.selectAgent);

  useEffect(() => {
    triggerRef.current = (document.activeElement as HTMLElement | null) ?? null;
    cancelButtonRef.current?.focus();
    return () => {
      // Return focus to the element that opened the modal. Guarded
      // because the trigger may have been unmounted (e.g. plan switch).
      const el = triggerRef.current;
      if (el && typeof el.focus === "function" && document.contains(el)) {
        el.focus();
      }
    };
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        if (!busy) onClose();
        return;
      }
      if (e.key !== "Tab") return;
      const root = dialogRef.current;
      if (!root) return;
      const items = Array.from(
        root.querySelectorAll<HTMLElement>(
          "button:not([disabled]), [href], input:not([disabled]), [tabindex]:not([tabindex='-1'])",
        ),
      ).filter((el) => !el.hasAttribute("data-focus-skip"));
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey) {
        if (active === first || !root.contains(active)) {
          e.preventDefault();
          last.focus();
        }
      } else if (active === last) {
        e.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const isHardConfirm = stage === "hard";
  const bodyText = isHardConfirm
    ? "Permanently deletes the plan file and cascades related rows. This cannot be undone."
    : `Moves the plan file to the archive and cascades related rows. Recoverable for ${retentionDays} day${retentionDays === 1 ? "" : "s"} from the Activity tab. Agents that ran for this plan stay in the agent list for historic reference.`;
  const heading = isHardConfirm
    ? `Permanently delete plan ${planName}?`
    : `Delete plan ${planName}?`;
  const primaryLabel = isHardConfirm
    ? busy
      ? "Deleting…"
      : "Permanently delete"
    : busy
      ? "Deleting…"
      : "Delete";
  // Only show the Shift hint when we're on the soft path AND the org
  // hasn't opted out of retention — otherwise the modifier does nothing.
  const showShiftHint = !isHardConfirm && retentionDays > 0;

  async function runDelete(hard: boolean) {
    setError(null);
    setRunningAgents(null);
    setBusy(true);
    try {
      await deletePlan(planName, hard ? { hard: true } : undefined);
      onClose();
    } catch (e) {
      if (e instanceof HttpError && e.status === 409) {
        const body = e.body as
          | PlanHasRunningAgentsBody
          | AutoModeInFlightBody
          | GenericErrorBody
          | undefined;
        if (body && body.error === "plan_has_running_agents") {
          const agents = (body as PlanHasRunningAgentsBody).agents ?? [];
          setRunningAgents(agents);
          setError("Cannot delete: this plan has running agents.");
        } else if (body && body.error === "auto_mode_in_flight") {
          setError(
            "Cannot delete: auto-mode is mid-flight (open fix attempt). Pause auto-mode or wait for it to settle.",
          );
        } else {
          setError(`Cannot delete: ${body?.error ?? "blocked"}`);
        }
      } else if (e instanceof HttpError && e.status === 404) {
        // Plan already gone (e.g. another tab raced this delete). The
        // sidebar will catch up via the WS event; treat as success so
        // the user isn't left staring at an unactionable error.
        onClose();
        return;
      } else {
        const msg = e instanceof Error ? e.message : String(e);
        setError(`Delete failed: ${msg}`);
      }
    } finally {
      setBusy(false);
    }
  }

  function onPrimary(e: React.MouseEvent<HTMLButtonElement>) {
    if (busy) return;
    if (isHardConfirm) {
      runDelete(true);
      return;
    }
    if (e.shiftKey) {
      // Stage transition only — the Shift modifier doesn't issue the
      // delete on its own. The user confirms by clicking the
      // "Permanently delete" button on the re-rendered modal.
      setStage("hard");
      setError(null);
      setRunningAgents(null);
      return;
    }
    runDelete(false);
  }

  function onAgentClick(id: string) {
    selectAgent(id);
    onClose();
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onClick={() => !busy && onClose()}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descId}
        className="bg-gray-900 border border-gray-700 rounded-md shadow-xl p-5 w-full max-w-md"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id={titleId} className="text-base font-semibold text-gray-100">
          {heading}
        </h2>
        <p id={descId} className="mt-2 text-sm text-gray-300">
          {bodyText}
        </p>
        {showShiftHint && (
          <p className="mt-2 text-xs text-gray-500">
            Hold <kbd className="px-1 py-0.5 rounded border border-gray-700 bg-gray-800 font-mono text-[10px] text-gray-300">Shift</kbd>{" "}
            while clicking Delete to permanently delete (skip archive).
          </p>
        )}
        {error && (
          <div
            role="alert"
            className="mt-3 rounded border border-red-700/50 bg-red-900/20 px-3 py-2 text-xs text-red-200"
          >
            <div>{error}</div>
            {runningAgents && runningAgents.length > 0 && (
              <ul className="mt-2 space-y-1">
                {runningAgents.map((id) => (
                  <li key={id}>
                    <button
                      type="button"
                      onClick={() => onAgentClick(id)}
                      className="font-mono text-red-100 underline decoration-dotted hover:text-white"
                      title="Open this agent's terminal"
                    >
                      {id}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        <div className="mt-5 flex items-center justify-end gap-2">
          <button
            ref={cancelButtonRef}
            type="button"
            onClick={onClose}
            disabled={busy}
            className="px-3 py-1.5 text-xs text-gray-300 hover:text-gray-100 disabled:opacity-50 transition"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onPrimary}
            disabled={busy}
            className="px-3 py-1.5 text-xs bg-red-700 hover:bg-red-600 disabled:opacity-50 text-white rounded transition"
          >
            {primaryLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
