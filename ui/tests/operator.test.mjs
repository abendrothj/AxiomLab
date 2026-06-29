import test from "node:test";
import assert from "node:assert/strict";
import { formatDeadline, routeFromHash } from "../src/operator.js";

test("routeFromHash accepts known routes and falls back safely", () => {
  assert.equal(routeFromHash("#/approvals"), "approvals");
  assert.equal(routeFromHash("#audit"), "audit");
  assert.equal(routeFromHash("#/unknown"), "overview");
  assert.equal(routeFromHash(""), "overview");
});

test("formatDeadline shows expired, seconds, and minutes", () => {
  assert.equal(formatDeadline(99, 100), "expired");
  assert.equal(formatDeadline(125, 100), "25s remaining");
  assert.equal(formatDeadline(221, 100), "3m remaining");
});
