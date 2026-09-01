import type { GraphSnapshot } from "../types/snapshot";

export async function loadSnapshot(): Promise<GraphSnapshot> {
  if (window.__GRIBOVIK_SNAPSHOT__) {
    return window.__GRIBOVIK_SNAPSHOT__;
  }
  if (__GRIBOVIK_EXPORT__) {
    throw new Error("Snapshot not found in window.__GRIBOVIK_SNAPSHOT__");
  }
  const response = await fetch("/api/graph");
  if (!response.ok) {
    throw new Error(`GET /api/graph failed: ${response.status}`);
  }
  return response.json();
}
