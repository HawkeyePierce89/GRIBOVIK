import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { GraphSnapshot } from "../types/snapshot";
import { loadSnapshot } from "./snapshot";

describe("loadSnapshot", () => {
  let originalFetch: typeof globalThis.fetch;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    if (typeof window === "undefined") {
      (globalThis as any).window = {};
    }
    delete window.__GRIBOVIK_SNAPSHOT__;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    delete (globalThis as any).window;
    vi.restoreAllMocks();
  });

  it("returns window.__GRIBOVIK_SNAPSHOT__ when present", async () => {
    const dummySnapshot = { meta: { repo: "test" } } as GraphSnapshot;
    window.__GRIBOVIK_SNAPSHOT__ = dummySnapshot;

    // We can explicitly test the behavior regardless of __GRIBOVIK_EXPORT__
    // by mocking it, but it should return the globalThis anyway.
    const result = await loadSnapshot();
    expect(result).toBe(dummySnapshot);
  });

  it("falls back to fetch(/api/graph) when globalThis absent", async () => {
    // Vitest runs in Node, where __GRIBOVIK_EXPORT__ is undefined in test context,
    // unless mocked. But let's assume the test environment will evaluate it.
    // In our tsconfig, we declared it. We can just mock fetch.
    const dummySnapshot = { meta: { repo: "fetched" } } as GraphSnapshot;
    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => dummySnapshot,
    });

    // Define __GRIBOVIK_EXPORT__ as false for this test
    vi.stubGlobal("__GRIBOVIK_EXPORT__", false);

    const result = await loadSnapshot();
    expect(result).toBe(dummySnapshot);
    expect(globalThis.fetch).toHaveBeenCalledWith("/api/graph");
  });

  it("rejects when fetch fails", async () => {
    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 404,
    });

    vi.stubGlobal("__GRIBOVIK_EXPORT__", false);

    await expect(loadSnapshot()).rejects.toThrow("GET /api/graph failed: 404");
  });

  it("rejects when globalThis absent and __GRIBOVIK_EXPORT__ is true", async () => {
    vi.stubGlobal("__GRIBOVIK_EXPORT__", true);

    await expect(loadSnapshot()).rejects.toThrow("Snapshot not found in window.__GRIBOVIK_SNAPSHOT__");
  });
});
