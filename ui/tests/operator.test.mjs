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

test("token storage defaults to session and supports remembered tokens", async () => {
  const makeStorage = () => {
    const values = new Map();
    return {
      getItem: (key) => values.get(key) || null,
      setItem: (key, value) => values.set(key, value),
      removeItem: (key) => values.delete(key),
    };
  };
  globalThis.sessionStorage = makeStorage();
  globalThis.localStorage = makeStorage();

  const { getToken, setToken, tokenIsRemembered } = await import("../src/api.js");
  setToken("session-token");
  assert.equal(getToken(), "session-token");
  assert.equal(tokenIsRemembered(), false);

  setToken("remembered-token", true);
  assert.equal(getToken(), "remembered-token");
  assert.equal(tokenIsRemembered(), true);

  setToken("");
  assert.equal(getToken(), "");
});
