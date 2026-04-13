import { create } from "zustand";
import { fetchJson, putJson } from "../api.js";

export interface PlanTask {
  number: string;
  title: string;
  description: string;
  filePaths: string[];
  acceptance: string;
  dependencies?: string[];
  status?: string;
  statusUpdatedAt?: string;
  agentId?: string;
}

export interface PlanPhase {
  number: number;
  title: string;
  description: string;
  tasks: PlanTask[];
}

export interface ParsedPlan {
  name: string;
  filePath: string;
  title: string;
  context: string;
  project: string | null;
  createdAt: string;
  modifiedAt: string;
  phases: PlanPhase[];
}

export interface PlanSummary {
  name: string;
  title: string;
  project: string | null;
  phaseCount: number;
  taskCount: number;
  doneCount: number;
  createdAt: string;
  modifiedAt: string;
}

export interface PlanWarning {
  name: string;
  file: string;
  error: string;
  timestamp: number;
}

interface PlanStore {
  plans: PlanSummary[];
  selectedPlan: ParsedPlan | null;
  loading: boolean;
  warnings: PlanWarning[];
  fetchPlans: () => Promise<void>;
  selectPlan: (name: string) => Promise<void>;
  updatePlan: (plan: ParsedPlan) => void;
  savePlan: (plan: ParsedPlan) => Promise<void>;
  addWarning: (w: PlanWarning) => void;
  dismissWarning: (name: string) => void;
}

export const usePlanStore = create<PlanStore>((set, get) => ({
  plans: [],
  selectedPlan: null,
  loading: false,
  warnings: [],

  fetchPlans: async () => {
    set({ loading: true });
    const plans = await fetchJson<PlanSummary[]>("/api/plans");
    set({ plans, loading: false });
  },

  selectPlan: async (name: string) => {
    set({ loading: true });
    const plan = await fetchJson<ParsedPlan>(`/api/plans/${name}`);
    set({ selectedPlan: plan, loading: false });
  },

  updatePlan: (plan: ParsedPlan) => {
    const { selectedPlan } = get();
    if (selectedPlan?.name === plan.name) {
      set({ selectedPlan: plan });
    }
  },

  savePlan: async (plan: ParsedPlan) => {
    await putJson(`/api/plans/${plan.name}`, {
      title: plan.title,
      context: plan.context,
      project: plan.project,
      phases: plan.phases.map((p) => ({
        number: p.number,
        title: p.title,
        description: p.description,
        tasks: p.tasks.map((t) => ({
          number: t.number,
          title: t.title,
          description: t.description,
          filePaths: t.filePaths,
          acceptance: t.acceptance,
          dependencies: t.dependencies ?? [],
        })),
      })),
    });
    set({ selectedPlan: plan });
  },

  addWarning: (w: PlanWarning) => {
    set((s) => ({
      warnings: [
        ...s.warnings.filter((x) => x.name !== w.name),
        w,
      ],
    }));
  },

  dismissWarning: (name: string) => {
    set((s) => ({
      warnings: s.warnings.filter((w) => w.name !== name),
    }));
  },
}));
