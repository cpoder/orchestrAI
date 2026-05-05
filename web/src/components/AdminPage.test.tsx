import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import {
  AdminPage,
  clampRetentionDays,
  retentionPreview,
} from "./AdminPage.js";
import { useSettingsStore } from "../stores/settings-store.js";

beforeEach(() => {
  useSettingsStore.setState({
    effort: "high",
    skipPermissions: true,
    webhookUrl: null,
    planArchiveRetentionDays: 30,
    loaded: true,
    drivers: [],
    defaultDriver: "claude",
    setEffort: vi.fn().mockResolvedValue(undefined),
    setSkipPermissions: vi.fn().mockResolvedValue(undefined),
    setWebhookUrl: vi.fn().mockResolvedValue(undefined),
    setPlanArchiveRetentionDays: vi.fn().mockResolvedValue(undefined),
  });
});

afterEach(() => {
  cleanup();
});

describe("clampRetentionDays", () => {
  it("clamps above max to 365", () => {
    expect(clampRetentionDays("9999", 30)).toBe(365);
  });

  it("clamps below min to 0", () => {
    expect(clampRetentionDays("-5", 30)).toBe(0);
  });

  it("returns parsed value within range", () => {
    expect(clampRetentionDays("60", 30)).toBe(60);
  });

  it("falls back when input is non-numeric", () => {
    expect(clampRetentionDays("abc", 42)).toBe(42);
  });

  it("falls back when input is empty", () => {
    expect(clampRetentionDays("   ", 7)).toBe(7);
  });
});

describe("retentionPreview", () => {
  it("renders permanent copy at 0", () => {
    expect(retentionPreview(0)).toMatch(/permanent/i);
  });

  it("renders singular at 1", () => {
    expect(retentionPreview(1)).toBe(
      "Soft-deleted plans are kept for 1 day.",
    );
  });

  it("renders plural at 30", () => {
    expect(retentionPreview(30)).toBe(
      "Soft-deleted plans are kept for 30 days.",
    );
  });
});

describe("AdminPage retention input", () => {
  it("seeds the input from the store value", () => {
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    expect(input.value).toBe("30");
    expect(screen.getByTestId("retention-preview").textContent).toMatch(
      /kept for 30 days/i,
    );
  });

  it("updates the live preview chip as the user types", () => {
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "0" } });
    expect(screen.getByTestId("retention-preview").textContent).toMatch(
      /permanent/i,
    );
    fireEvent.change(input, { target: { value: "7" } });
    expect(screen.getByTestId("retention-preview").textContent).toMatch(
      /kept for 7 days/i,
    );
  });

  it("clamps 9999 to 365 on blur and saves", async () => {
    const setRetention = vi.fn().mockResolvedValue(undefined);
    useSettingsStore.setState({ setPlanArchiveRetentionDays: setRetention });
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "9999" } });
    fireEvent.blur(input);
    await waitFor(() => expect(setRetention).toHaveBeenCalledWith(365));
    expect(input.value).toBe("365");
  });

  it("clamps -5 to 0 on blur and saves", async () => {
    const setRetention = vi.fn().mockResolvedValue(undefined);
    useSettingsStore.setState({ setPlanArchiveRetentionDays: setRetention });
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "-5" } });
    fireEvent.blur(input);
    await waitFor(() => expect(setRetention).toHaveBeenCalledWith(0));
    expect(input.value).toBe("0");
  });

  it("does not save when blur produces the same value as the store", async () => {
    const setRetention = vi.fn().mockResolvedValue(undefined);
    useSettingsStore.setState({ setPlanArchiveRetentionDays: setRetention });
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "30" } });
    fireEvent.blur(input);
    // Give the click handler a microtask tick.
    await Promise.resolve();
    expect(setRetention).not.toHaveBeenCalled();
  });

  it("surfaces an error message when the store action throws", async () => {
    const setRetention = vi
      .fn()
      .mockRejectedValue(new Error("network down"));
    useSettingsStore.setState({ setPlanArchiveRetentionDays: setRetention });
    render(<AdminPage />);
    const input = screen.getByTestId("retention-input") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "60" } });
    fireEvent.blur(input);
    await waitFor(() =>
      expect(screen.getByText(/network down/i)).toBeTruthy(),
    );
  });
});
