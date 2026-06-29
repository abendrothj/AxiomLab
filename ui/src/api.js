let csrfToken = "";

async function request(method, path, body) {
  const headers = {};
  if (body) headers["content-type"] = "application/json";
  if (method !== "GET" && csrfToken) headers["x-csrf-token"] = csrfToken;
  const response = await fetch(path, { method, headers, credentials: "same-origin", body: body ? JSON.stringify(body) : undefined });
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? {} : response.json();
}

export const api = {
  me: async () => { const principal = await request("GET", "/api/auth/me"); csrfToken = principal.csrf_token; return principal; },
  loginUrl: (returnTo = "/") => `/api/auth/login?return_to=${encodeURIComponent(returnTo)}`,
  devLogin: async (subject, role) => { const principal = await request("POST", "/api/auth/dev-login", { subject, role }); csrfToken = principal.csrf_token; return principal; },
  logout: async () => { await request("POST", "/api/auth/logout"); csrfToken = ""; },
  status: () => request("GET", "/api/status"), audit: (limit = 25) => request("GET", `/api/audit?limit=${limit}`),
  verifyAudit: () => request("POST", "/api/audit/verify"), agenda: () => request("GET", "/api/agenda"),
  queue: () => request("GET", "/api/queue"), pushDirective: (directive) => request("POST", "/api/queue", { directive }),
  cancelQueued: (id) => request("DELETE", `/api/queue/${id}`),
  reconcile: (id, retry, notes) => request("POST", `/api/queue/${id}/reconcile`, { retry, notes }),
  approvals: () => request("GET", "/api/approvals"), resolveApproval: (id, approved, notes) => request("POST", `/api/approvals/${id}`, { approved, notes }),
  lab: () => request("GET", "/api/lab"),
};
