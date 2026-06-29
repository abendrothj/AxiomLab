// Thin fetch wrappers over the server API. The optional bearer token (stored in
// localStorage) is sent on mutating calls; POST /api/queue requires it when the
// server has AXIOMLAB_JWT_SECRET configured.

export const getToken = () => localStorage.getItem("axiomlab_token") || "";
export const setToken = (token) => {
  if (token) localStorage.setItem("axiomlab_token", token);
  else localStorage.removeItem("axiomlab_token");
};

async function get(path) {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}

async function send(method, path, body) {
  const headers = { "content-type": "application/json" };
  const token = getToken();
  if (token) headers.authorization = `Bearer ${token}`;
  const r = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined });
  if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
  return r.json().catch(() => ({}));
}

export const api = {
  status: () => get("/api/status"),
  audit: (limit = 25) => get(`/api/audit?limit=${limit}`),
  verifyAudit: () => send("POST", "/api/audit/verify"),
  agenda: () => get("/api/agenda"),
  queue: () => get("/api/queue"),
  pushDirective: (directive) => send("POST", "/api/queue", { directive }),
  cancelQueued: (id) => send("DELETE", `/api/queue/${id}`),
  approvals: () => get("/api/approvals"),
  resolveApproval: (id, approved, notes, approverId = "operator") =>
    send("POST", `/api/approvals/${id}`, { approved, notes, approver_id: approverId }),
  lab: () => get("/api/lab"),
};
