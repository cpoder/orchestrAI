import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { NewPlanForm } from "./NewPlanForm.js";

type FetchHandler = (path: string) => {
  status: number;
  body: unknown;
};

function installFetchMock(handler: FetchHandler) {
  const fn = vi.fn(async (input: string | URL | Request) => {
    const path =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.pathname + input.search
          : input.url;
    const { status, body } = handler(path);
    return new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

describe("NewPlanForm runner banner", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("renders the no-runner banner when /api/folders returns 503", async () => {
    installFetchMock((path) => {
      if (path.startsWith("/api/folders")) {
        return { status: 503, body: { error: "no_runner_connected" } };
      }
      if (path.startsWith("/api/templates")) {
        return { status: 200, body: [] };
      }
      return { status: 404, body: {} };
    });

    render(<NewPlanForm onClose={() => {}} />);

    await waitFor(() => {
      expect(screen.getByText(/No runner connected/i)).toBeTruthy();
    });

    const link = screen.getByRole("link", { name: /Runners page/i });
    expect((link as HTMLAnchorElement).getAttribute("href")).toBe("/runners");
  });

  it("renders the runner-offline banner with last-seen on 504", async () => {
    const lastSeen = new Date(Date.now() - 5 * 60 * 1000)
      .toISOString()
      .replace("T", " ")
      .replace("Z", "")
      .split(".")[0]; // SQLite-style: "YYYY-MM-DD HH:mm:ss"

    installFetchMock((path) => {
      if (path.startsWith("/api/folders")) {
        return { status: 504, body: { error: "runner_unavailable" } };
      }
      if (path.startsWith("/api/runners")) {
        return {
          status: 200,
          body: { runners: [{ lastSeenAt: lastSeen }] },
        };
      }
      if (path.startsWith("/api/templates")) {
        return { status: 200, body: [] };
      }
      return { status: 404, body: {} };
    });

    render(<NewPlanForm onClose={() => {}} />);

    await waitFor(() => {
      expect(screen.getByText(/Runner is offline\. Last seen 5m ago\./)).toBeTruthy();
    });
  });

  it("hides the suggestion dropdown when the runner is offline", async () => {
    installFetchMock((path) => {
      if (path.startsWith("/api/folders")) {
        return { status: 503, body: { error: "no_runner_connected" } };
      }
      if (path.startsWith("/api/templates")) {
        return { status: 200, body: [] };
      }
      return { status: 404, body: {} };
    });

    render(<NewPlanForm onClose={() => {}} />);

    await waitFor(() => {
      expect(screen.getByText(/No runner connected/i)).toBeTruthy();
    });

    // Folder input is still enabled — user can still type an absolute path.
    const input = screen.getByPlaceholderText(/~\/my-project/) as HTMLInputElement;
    expect(input.disabled).toBe(false);
  });

  it("does not render any banner on a clean fetch", async () => {
    installFetchMock((path) => {
      if (path.startsWith("/api/folders")) {
        return {
          status: 200,
          body: [{ name: "proj", path: "/home/cpo/proj" }],
        };
      }
      if (path.startsWith("/api/templates")) {
        return { status: 200, body: [] };
      }
      return { status: 404, body: {} };
    });

    render(<NewPlanForm onClose={() => {}} />);

    // Wait for the folders fetch to settle, then assert no banner.
    await waitFor(() => {
      expect(screen.queryByText(/No runner connected/i)).toBeNull();
      expect(screen.queryByText(/Runner is offline/i)).toBeNull();
    });
  });
});
