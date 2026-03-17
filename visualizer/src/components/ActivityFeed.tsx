import { ToolExecutionEvent } from "../types";

interface Props {
  events: ToolExecutionEvent[];
}

// Human-readable summaries — no raw params, no technical IDs
function summarise(tool: string, params: Record<string, unknown>, status: string): string {
  const ok = status === "success";
  switch (tool) {
    case "move_arm":       return ok ? "Arm repositioned" : "Arm move out of range";
    case "dispense": {
      const vol = params["volume_ul"];
      return ok
        ? `Dispensed ${vol}µL`
        : `Dispense rejected — volume exceeds safe limit`;
    }
    case "aspirate": {
      const vol = params["volume_ul"];
      return ok ? `Aspirated ${vol}µL` : "Aspirate rejected";
    }
    case "transfer":       return ok ? "Liquid transferred" : "Transfer rejected";
    case "mix":            return ok ? `Mixed at ${params["rpm"]} rpm` : "Mix rejected";
    case "grip":           return ok ? "Labware gripped" : "Grip failed";
    case "centrifuge":     return ok ? `Centrifuge at ${params["rpm"]} rpm` : "Centrifuge rejected";
    case "read_absorbance":return ok ? `Absorbance measured` : "Absorbance read failed";
    case "read_ph":        return ok ? "pH measured" : "pH read failed";
    case "read_temperature":return ok ? "Temperature read" : "Temperature read failed";
    case "read_sensor":    return ok ? "Sensor read" : "Sensor read failed";
    case "set_temperature":return ok ? `Temperature set` : "Temperature rejected — out of range";
    case "set_pressure":   return ok ? "Pressure set" : "Pressure rejected — out of range";
    case "set_stir_rate":  return ok ? `Stir rate set` : "Stir rate rejected";
    default:               return ok ? "Action completed" : "Action rejected";
  }
}

const TOOL_LABELS: Record<string, string> = {
  move_arm:        "ARM",
  dispense:        "PUMP",
  aspirate:        "PUMP",
  transfer:        "TRANSFER",
  mix:             "MIX",
  grip:            "GRIP",
  centrifuge:      "CENTRIFUGE",
  read_absorbance: "SPECTRO",
  read_ph:         "pH PROBE",
  read_temperature:"THERMOMETER",
  read_sensor:     "SENSOR",
  set_temperature: "HEATER",
  set_pressure:    "PRESSURE",
  set_stir_rate:   "STIRRER",
};

export default function ActivityFeed({ events }: Props) {
  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      }}
    >
      <div
        style={{
          padding: "6px 14px",
          fontSize: 9,
          color: "#1a3a4a",
          letterSpacing: "0.14em",
          borderBottom: "1px solid #0d1a25",
          flexShrink: 0,
        }}
      >
        RECENT ACTIVITY
      </div>

      <div style={{ flex: 1, overflowY: "auto" }}>
        {events.length === 0 ? (
          <div style={{ padding: "16px 14px", color: "#1a3a2a", fontSize: 10 }}>
            no activity yet
          </div>
        ) : (
          events.map((ev, i) => {
            const ok = ev.status === "success";
            return (
              <div
                key={i}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  padding: "7px 14px",
                  borderBottom: "1px solid #0a1218",
                }}
              >
                {/* Status dot */}
                <span
                  style={{
                    width: 6,
                    height: 6,
                    borderRadius: "50%",
                    background: ok ? "#00ff9d" : "#ff3b3b",
                    flexShrink: 0,
                    boxShadow: ok ? "0 0 4px #00ff9d66" : "0 0 4px #ff3b3b66",
                  }}
                />

                {/* Tool label badge */}
                <span
                  style={{
                    fontSize: 9,
                    color: ok ? "#2a6a5a" : "#6a2a2a",
                    letterSpacing: "0.08em",
                    minWidth: 80,
                    flexShrink: 0,
                  }}
                >
                  {TOOL_LABELS[ev.tool] ?? ev.tool.toUpperCase()}
                </span>

                {/* Human summary */}
                <span
                  style={{
                    fontSize: 11,
                    color: ok ? "#7ab09a" : "#a07070",
                    flex: 1,
                  }}
                >
                  {summarise(ev.tool, ev.params, ev.status)}
                </span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
