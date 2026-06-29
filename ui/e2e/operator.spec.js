import { expect, test } from "@playwright/test";

const baseState = {
  status: { running: false, backend: "simulator", queue: 0, pending_approvals: 0 },
  audit: { verified: true, total: 0, tip_hash: null, entries: [] },
  approvals: [],
  queue: [],
  agenda: [],
  lab: { reagents: {}, vessel_contents: {} },
};

async function mockApi(page, overrides = {}) {
  const state = { ...baseState, ...overrides };
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const method = route.request().method();
    if (path === "/api/status") return route.fulfill({ json: state.status });
    if (path === "/api/audit") return route.fulfill({ json: state.audit });
    if (path === "/api/approvals" && method === "GET") return route.fulfill({ json: state.approvals });
    if (path === "/api/queue" && method === "GET") return route.fulfill({ json: state.queue });
    if (path === "/api/agenda") return route.fulfill({ json: state.agenda });
    if (path === "/api/lab") return route.fulfill({ json: state.lab });
    if (path === "/api/queue" && method === "POST") {
      state.queue.push({ id: "run-1", directive: route.request().postDataJSON().directive, status: "pending", created_secs: Math.floor(Date.now() / 1000), summary: null });
      state.status.queue = state.queue.length;
      return route.fulfill({ status: 202, json: { id: "run-1" } });
    }
    if (path.startsWith("/api/approvals/") && method === "POST") {
      state.lastDecision = route.request().postDataJSON();
      state.approvals = [];
      state.status.pending_approvals = 0;
      return route.fulfill({ json: { resolved: path.split("/").pop() } });
    }
    return route.fulfill({ status: 404, body: "not mocked" });
  });
  return state;
}

test("operator navigates routes and queues a directive", async ({ page }) => {
  const state = await mockApi(page);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Command center" })).toBeVisible();
  await page.getByRole("link", { name: "Runs" }).click();
  await expect(page).toHaveURL(/#\/runs$/);
  await page.getByPlaceholder("Describe the lab objective. The LLM proposes; gates enforce.").fill("Read tube_1 at 500 nm");
  await page.getByRole("button", { name: "Queue directive" }).click();
  await expect(page.getByText("Read tube_1 at 500 nm", { exact: true })).toBeVisible();
  expect(state.queue).toHaveLength(1);
});

test("operator reviews exact scope and denies an approval", async ({ page }) => {
  const approval = {
    id: "approval-1", tool: "move_arm", params: { x: 10, y: 20, z: 5 },
    scope_hash: "abcdef0123456789abcdef", created_secs: Math.floor(Date.now() / 1000),
    expires_secs: Math.floor(Date.now() / 1000) + 300, risk_class: "actuation",
    gate: "ApprovalGate", reason: "Physical movement requires review",
  };
  const state = await mockApi(page, { approvals: [approval], status: { ...baseState.status, pending_approvals: 1 } });
  await page.goto("/#/approvals");
  await expect(page.getByRole("heading", { name: "move_arm" })).toBeVisible();
  await expect(page.getByText("Physical movement requires review", { exact: true })).toBeVisible();
  await page.getByPlaceholder("approver id").fill("alice");
  await page.getByPlaceholder("decision notes").fill("Unexpected position");
  await page.getByRole("button", { name: "Deny" }).click();
  await expect(page.getByText("No pending approvals.", { exact: false })).toBeVisible();
  expect(state.lastDecision).toEqual({ approved: false, notes: "Unexpected position", approver_id: "alice" });
});
