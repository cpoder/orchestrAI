import { useState } from "react";
import { usePlanStore } from "../stores/plan-store.js";
import { postJson } from "../api.js";
import { PhaseColumn } from "./PhaseColumn.js";
import { EditableText } from "./EditableText.js";

interface SyncResult {
  summary: { total: number; completed: number; in_progress: number; pending: number };
}

export function PlanBoard() {
  const plan = usePlanStore((s) => s.selectedPlan);
  const loading = usePlanStore((s) => s.loading);
  const selectPlan = usePlanStore((s) => s.selectPlan);
  const [syncing, setSyncing] = useState(false);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [converting, setConverting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fetchPlans = usePlanStore((s) => s.fetchPlans);
  const savePlan = usePlanStore((s) => s.savePlan);

  const isMd = plan?.filePath?.endsWith(".md") ?? false;

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500">
        Loading...
      </div>
    );
  }

  if (!plan) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-center">
          <div className="text-4xl mb-3 text-gray-700">&#9776;</div>
          <p className="text-gray-500">Select a plan from the sidebar</p>
          <p className="text-xs text-gray-600 mt-1">
            Plans are loaded from ~/.claude/plans/
          </p>
        </div>
      </div>
    );
  }

  // Aggregate stats
  const allTasks = plan.phases.flatMap((p) => p.tasks);
  const total = allTasks.length;
  const done = allTasks.filter(
    (t) => t.status === "completed" || t.status === "skipped"
  ).length;
  const inProgress = allTasks.filter((t) => t.status === "in_progress").length;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  async function handleSync() {
    setSyncing(true);
    setSyncResult(null);
    setError(null);
    try {
      const result = await postJson<SyncResult>(
        `/api/plans/${plan!.name}/auto-status`,
        {}
      );
      setSyncResult(result);
      // Refresh plan to get updated statuses
      await selectPlan(plan!.name);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Sync failed: ${msg}`);
      console.error("Sync failed:", e);
    } finally {
      setSyncing(false);
    }
  }

  async function saveField(patch: Partial<typeof plan>) {
    if (!plan) return;
    const updated = { ...plan, ...patch };
    try {
      await savePlan(updated);
      await fetchPlans();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Save failed: ${msg}`);
    }
  }

  async function handleConvert() {
    setConverting(true);
    setError(null);
    try {
      await postJson(`/api/plans/${plan!.name}/convert`, {});
      await fetchPlans();
      await selectPlan(plan!.name);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(`Convert failed: ${msg}`);
      console.error("Convert failed:", e);
    } finally {
      setConverting(false);
    }
  }

  return (
    <div className="p-6">
      {/* Plan header */}
      <div className="mb-6">
        <div className="flex items-center gap-3 mb-1 text-xs">
          {plan.project && (
            <span className="text-indigo-400 font-medium flex items-center gap-1.5">
              <span className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
              {plan.project}
            </span>
          )}
          <span className="text-gray-600">
            Created {new Date(plan.createdAt).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" })}
            {plan.modifiedAt !== plan.createdAt && (
              <> / Modified {new Date(plan.modifiedAt).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" })}</>
            )}
          </span>
          {isMd && (
            <span className="text-amber-500/60 font-mono">.md</span>
          )}
        </div>
        <div className="flex items-center gap-3">
          <h2 className="text-xl font-bold">
            <EditableText
              value={plan.title}
              onSave={(v) => saveField({ title: v })}
              className="text-xl font-bold"
              editClassName="text-xl font-bold"
            />
            <span className="text-sm font-mono font-normal text-gray-600 ml-2">{plan.name}</span>
          </h2>
          <span className="text-xs text-gray-500 bg-gray-800 px-2 py-0.5 rounded">
            {done}/{total} tasks done ({pct}%)
            {inProgress > 0 && (
              <span className="text-amber-400 ml-1"> | {inProgress} in progress</span>
            )}
          </span>
        </div>
        <div className="flex items-center gap-3 mt-2">
          <div className="text-sm text-gray-400 max-w-3xl flex-1">
            <EditableText
              value={plan.context}
              onSave={(v) => saveField({ context: v })}
              multiline
              className="line-clamp-2"
              editClassName="text-sm"
              placeholder="Add context..."
            />
          </div>
          {isMd && (
            <button
              onClick={handleConvert}
              disabled={converting}
              className="flex-shrink-0 px-3 py-1.5 text-xs bg-gray-800 border border-gray-700 hover:border-amber-600 hover:text-amber-400 disabled:opacity-50 text-gray-300 rounded transition"
              title="Convert this plan from Markdown to YAML format"
            >
              {converting ? "Converting..." : "Convert to YAML"}
            </button>
          )}
          <button
            onClick={handleSync}
            disabled={syncing || !plan.project}
            className="flex-shrink-0 px-3 py-1.5 text-xs bg-gray-800 border border-gray-700 hover:border-indigo-600 hover:text-indigo-400 disabled:opacity-50 disabled:hover:border-gray-700 disabled:hover:text-gray-400 text-gray-300 rounded transition"
            title={plan.project ? "Scan project files and git history to detect task statuses" : "Set a project first to enable auto-detection"}
          >
            {syncing ? "Scanning..." : "Sync Status"}
          </button>
        </div>
        {/* Error toast */}
        {error && (
          <div className="mt-2 text-xs text-red-400 bg-red-900/20 border border-red-800/30 rounded px-3 py-2 inline-flex items-center gap-2">
            <span>{error}</span>
            <button onClick={() => setError(null)} className="text-red-600 hover:text-red-400 ml-2">
              dismiss
            </button>
          </div>
        )}
        {/* Sync result toast */}
        {syncResult && (
          <div className="mt-2 text-xs text-gray-400 bg-gray-800/50 border border-gray-700 rounded px-3 py-2 inline-flex items-center gap-3">
            <span className="text-emerald-400">{syncResult.summary.completed} done</span>
            <span className="text-amber-400">{syncResult.summary.in_progress} active</span>
            <span className="text-gray-500">{syncResult.summary.pending} pending</span>
            <button onClick={() => setSyncResult(null)} className="text-gray-600 hover:text-gray-400 ml-2">
              dismiss
            </button>
          </div>
        )}
        {/* Overall progress */}
        {total > 0 && (
          <div className="mt-3 h-1.5 bg-gray-800 rounded-full overflow-hidden max-w-md">
            <div
              className="h-full bg-emerald-500 rounded-full transition-all duration-300"
              style={{ width: `${pct}%` }}
            />
          </div>
        )}
      </div>

      {/* Phase columns -- horizontal scroll */}
      <div className="flex gap-4 overflow-x-auto pb-4">
        {plan.phases.map((phase) => (
          <PhaseColumn key={phase.number} phase={phase} planName={plan.name} />
        ))}
      </div>
    </div>
  );
}
