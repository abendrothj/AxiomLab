import { ToolExecutionEvent } from "../types";

interface Props {
  events: ToolExecutionEvent[];
}

function formatParams(tool: string, params: Record<string, unknown>): string {
  // Show the most relevant param(s) per tool type
  switch (tool) {
    case "dispense":
    case "aspirate": {
      const vol = params["volume_ul"];
      const pump = params["pump_id"];
      return vol !== undefined ? `${vol} µL${pump ? ` → ${pump}` : ""}` : JSON.stringify(params).slice(0, 40);
    }
    case "move_arm":
      return `(${params["x"] ?? 0}, ${params["y"] ?? 0}, ${params["z"] ?? 0}) mm`;
    case "read_absorbance":
      return `${params["wavelength_nm"] ?? "?"} nm`;
    case "set_temperature":
    case "read_temperature": {
      const mk = params["target_mk"];
      if (mk !== undefined) {
        const c = ((mk as number) / 1000 - 273.15).toFixed(1);
        return `${c} °C`;
      }
      return JSON.stringify(params).slice(0, 40);
    }
    case "stir":
    case "centrifuge":
      return `${params["rpm"] ?? "?"} rpm`;
    case "read_ph":
      return params["vessel_id"] ? `vessel:${params["vessel_id"]}` : "probe";
    case "read_pressure":
      return params["vessel_id"] ? `vessel:${params["vessel_id"]}` : "ambient";
    case "add_reagent":
      return `${params["reagent_id"] ?? "?"} × ${params["quantity_mg"] ?? "?"}mg`;
    case "filter":
      return `vessel:${params["vessel_id"] ?? "?"}`;
    case "seal_vessel":
    case "unseal_vessel":
      return `vessel:${params["vessel_id"] ?? "?"}`;
    case "image_capture":
      return `target:${params["target"] ?? "?"}`;
    default:
      return JSON.stringify(params).slice(0, 40);
  }
}

export default function ToolEventFeed({ events }: Props) {
  const isEmpty = events.length === 0;

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        background: "#0a0d12",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      }}
    >
      {/* Panel label */}
      <div
        style={{
          padding: "4px 10px",
          fontSize: 9,
          color: "#1a3a4a",
          letterSpacing: "0.14em",
          borderBottom: "1px solid #0d1a25",
          flexShrink: 0,
        }}
      >
        TOOL EVENT FEED
      </div>

      {/* Event rows */}
      <div style={{ flex: 1, overflowY: "auto", padding: "3px 0" }}>
        {isEmpty ? (
          <div style={{ padding: "8px 12px", color: "#1a3a2a", fontSize: 10 }}>
            awaiting tool calls...
          </div>
        ) : (
          events.map((ev, i) => {
            const success = ev.status === "success";
            const statusColor = success ? "#00ff9d" : "#ff3b3b";
            const statusIcon = success ? "✓" : "✗";
            const dimColor = "#3a4a5a";

            return (
              <div
                key={i}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  padding: "3px 10px",
                  borderBottom: "1px solid #0d1218",
                  fontSize: 10,
                  lineHeight: 1.4,
                }}
              >
                {/* Status icon */}
                <span
                  style={{
                    color: statusColor,
                    fontWeight: 700,
                    flexShrink: 0,
                    width: 10,
                    textAlign: "center",
                  }}
                >
                  {statusIcon}
                </span>

                {/* Tool name */}
                <span
                  style={{
                    color: success ? "#00d4ff" : "#ff6666",
                    flexShrink: 0,
                    minWidth: 120,
                  }}
                >
                  {ev.tool}
                </span>

                {/* Target */}
                {ev.target && (
                  <span style={{ color: dimColor, flexShrink: 0, minWidth: 70 }}>
                    {ev.target.slice(0, 12)}
                  </span>
                )}

                {/* Params */}
                <span
                  style={{
                    color: "#2a4a3a",
                    flex: 1,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {formatParams(ev.tool, ev.params)}
                </span>

                {/* Status badge */}
                <span
                  style={{
                    color: statusColor,
                    fontSize: 9,
                    letterSpacing: "0.06em",
                    flexShrink: 0,
                  }}
                >
                  {success ? "OK" : "REJECTED"}
                </span>

                {/* Max safe limit (amber) — only shown when relevant */}
                {!success && ev.max_safe_limit > 0 && (
                  <span style={{ color: "#ffaa00", fontSize: 9, flexShrink: 0 }}>
                    ⌀{ev.max_safe_limit}
                  </span>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
