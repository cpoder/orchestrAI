import { create } from "zustand";
import { fetchJson, putJson } from "../api.js";

export type EffortLevel = "low" | "medium" | "high" | "max";

interface SettingsStore {
  effort: EffortLevel;
  loaded: boolean;
  fetchSettings: () => Promise<void>;
  setEffort: (level: EffortLevel) => Promise<void>;
}

export const useSettingsStore = create<SettingsStore>((set) => ({
  effort: "high",
  loaded: false,

  fetchSettings: async () => {
    const data = await fetchJson<{ effort: EffortLevel }>("/api/settings");
    set({ effort: data.effort, loaded: true });
  },

  setEffort: async (level) => {
    await putJson("/api/settings", { effort: level });
    set({ effort: level });
  },
}));
