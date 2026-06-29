export const routes = [
  ["overview", "Overview"],
  ["approvals", "Approvals"],
  ["runs", "Runs"],
  ["audit", "Audit"],
  ["lab", "Lab"],
  ["settings", "Settings"],
];

export function routeFromHash(hash) {
  const route = hash.replace(/^#\/?/, "") || "overview";
  return routes.some(([key]) => key === route) ? route : "overview";
}

export function formatDeadline(secs, nowSecs = Math.floor(Date.now() / 1000)) {
  if (!secs) return "—";
  const delta = secs - nowSecs;
  if (delta <= 0) return "expired";
  if (delta < 60) return `${delta}s remaining`;
  return `${Math.ceil(delta / 60)}m remaining`;
}
