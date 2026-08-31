import { describe, expect, it } from "vitest";

import type { ReviewState } from "../types/snapshot";
import {
  addComment,
  countStatuses,
  nodeIdsWithStatus,
  reviewFor,
  setStatus,
  statusOf,
} from "./review";

const STAMP = "2026-09-01T12:00:00.000Z";

describe("statusOf", () => {
  it("reports the recorded status", () => {
    const state: ReviewState = {
      a: { status: "approved", comments: [] },
      b: { status: "rejected", comments: [] },
    };

    expect(statusOf(state, "a")).toBe("approved");
    expect(statusOf(state, "b")).toBe("rejected");
  });

  it("treats a node with no entry as pending", () => {
    expect(statusOf({}, "never-seen")).toBe("pending");
    expect(reviewFor({}, "never-seen")).toEqual({
      status: "pending",
      comments: [],
    });
  });
});

describe("countStatuses", () => {
  it("counts nodes without a state entry as pending", () => {
    const state: ReviewState = {
      a: { status: "approved", comments: [] },
      b: { status: "rejected", comments: [] },
    };

    expect(countStatuses(state, ["a", "b", "c", "d"])).toEqual({
      approved: 1,
      rejected: 1,
      pending: 2,
    });
  });

  it("counts an explicit pending entry the same as a missing one", () => {
    const state: ReviewState = { a: { status: "pending", comments: [] } };

    expect(countStatuses(state, ["a", "b"])).toEqual({
      approved: 0,
      rejected: 0,
      pending: 2,
    });
  });

  it("ignores state for nodes outside the current graph", () => {
    const state: ReviewState = { gone: { status: "approved", comments: [] } };

    expect(countStatuses(state, ["here"])).toEqual({
      approved: 0,
      rejected: 0,
      pending: 1,
    });
  });

  it("returns all zeroes for an empty graph", () => {
    expect(countStatuses({}, [])).toEqual({
      approved: 0,
      rejected: 0,
      pending: 0,
    });
  });
});

describe("nodeIdsWithStatus", () => {
  it("selects the matching ids in the order given", () => {
    const state: ReviewState = {
      b: { status: "approved", comments: [] },
      c: { status: "approved", comments: [] },
    };

    expect(nodeIdsWithStatus(state, ["a", "b", "c"], "approved")).toEqual([
      "b",
      "c",
    ]);
    expect(nodeIdsWithStatus(state, ["a", "b", "c"], "pending")).toEqual(["a"]);
  });
});

describe("setStatus", () => {
  it("records a status for an untouched node", () => {
    expect(setStatus({}, "a", "approved")).toEqual({
      a: { status: "approved", comments: [] },
    });
  });

  it("does not mutate the input state", () => {
    const state: ReviewState = {};
    setStatus(state, "a", "rejected");
    expect(state).toEqual({});
  });

  it("keeps comments when the status changes", () => {
    const state = addComment({}, "a", "needs a test", STAMP);

    expect(setStatus(state, "a", "rejected")).toEqual({
      a: {
        status: "rejected",
        comments: [{ text: "needs a test", created_at: STAMP }],
      },
    });
  });

  it("drops the entry when returning to pending with no comments", () => {
    const state = setStatus({}, "a", "approved");

    expect(setStatus(state, "a", "pending")).toEqual({});
  });

  it("keeps a pending entry that still has comments", () => {
    const state = addComment(setStatus({}, "a", "approved"), "a", "why?", STAMP);

    expect(setStatus(state, "a", "pending")).toEqual({
      a: {
        status: "pending",
        comments: [{ text: "why?", created_at: STAMP }],
      },
    });
  });
});

describe("addComment", () => {
  it("appends to an untouched node, leaving it pending", () => {
    expect(addComment({}, "a", "first", STAMP)).toEqual({
      a: {
        status: "pending",
        comments: [{ text: "first", created_at: STAMP }],
      },
    });
  });

  it("appends in order and keeps the status", () => {
    const state = addComment(setStatus({}, "a", "rejected"), "a", "one", STAMP);
    const next = addComment(state, "a", "two", STAMP);

    expect(next).toEqual({
      a: {
        status: "rejected",
        comments: [
          { text: "one", created_at: STAMP },
          { text: "two", created_at: STAMP },
        ],
      },
    });
  });

  it("trims the text and ignores blank comments", () => {
    expect(addComment({}, "a", "  spaced  ", STAMP)).toEqual({
      a: {
        status: "pending",
        comments: [{ text: "spaced", created_at: STAMP }],
      },
    });
    expect(addComment({}, "a", "   ", STAMP)).toEqual({});
  });

  it("does not mutate the input state", () => {
    const state = addComment({}, "a", "one", STAMP);
    addComment(state, "a", "two", STAMP);

    expect(state["a"]?.comments).toHaveLength(1);
  });
});
