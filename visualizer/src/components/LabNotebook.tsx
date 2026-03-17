import { useEffect, useRef } from "react";
import { NotebookEntryEvent } from "../types";

interface Props {
  entries: NotebookEntryEvent[];
}

const OUTCOME_COLORS = {
  discovery: "#00ff9d",
  rejection: "#ff3b3b",
  inconclusive: "#6c757d",
} as const;

const OUTCOME_LABELS = {
  discovery: "DISCOVERY",
  rejection: "REJECTION",
  inconclusive: "INCONCLUSIVE",
} as const;

function formatElapsed(timestamp_ms: number): string {
  const elapsed = Date.now() - timestamp_ms;
  const secs = Math.floor(elapsed / 1000);
  if (secs < 60) return `+0:${String(secs).padStart(2, "0")}`;
  const mins = Math.floor(secs / 60);
  const s = secs % 60;
  return `+${mins}:${String(s).padStart(2, "0")}`;
}

export default function LabNotebook({ entries }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom when new entries arrive
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [entries.length]);

  const isEmpty = entries.length === 0;

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        background: "#0a0d12",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
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
        LAB NOTEBOOK
      </div>

      {/* Entry list */}
      <div
        ref={scrollRef}
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "6px 0",
        }}
      >
        {isEmpty ? (
          <div
            style={{
              padding: "20px 12px",
              color: "#1a3a2a",
              fontSize: 10,
              letterSpacing: "0.08em",
            }}
          >
            no entries yet — the AI documents findings here
          </div>
        ) : (
          entries.map((entry, i) => {
            const outcome = entry.outcome as keyof typeof OUTCOME_COLORS;
            const borderColor = OUTCOME_COLORS[outcome] ?? "#3a4a5a";
            const label = OUTCOME_LABELS[outcome] ?? "UNKNOWN";

            return (
              <div
                key={i}
                style={{
                  margin: "4px 8px",
                  padding: "8px 10px 8px 12px",
                  background: "#0f1117",
                  borderLeft: `3px solid ${borderColor}`,
                  borderRadius: "0 3px 3px 0",
                  animation: "slideInUp 0.2s ease-out",
                  fontFamily: '"JetBrains Mono", "Fira Code", monospace',
                }}
              >
                {/* Entry header */}
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    marginBottom: 5,
                    flexWrap: "wrap",
                  }}
                >
                  <span style={{ color: "#3a6a8a", fontSize: 9, letterSpacing: "0.06em" }}>
                    EXP-{entry.experiment_id.slice(0, 6).toUpperCase()}
                  </span>
                  <span style={{ color: "#1a3a4a", fontSize: 9 }}>
                    {formatElapsed(entry.timestamp_ms)}
                  </span>
                  {entry.tool_that_triggered && (
                    <span
                      style={{
                        color: "#1a4a5a",
                        fontSize: 9,
                        background: "#0d1a25",
                        padding: "1px 5px",
                        borderRadius: 2,
                      }}
                    >
                      {entry.tool_that_triggered}
                    </span>
                  )}
                  <span
                    style={{
                      marginLeft: "auto",
                      fontSize: 9,
                      color: borderColor,
                      letterSpacing: "0.06em",
                      fontWeight: 700,
                    }}
                  >
                    {label}
                  </span>
                </div>

                {/* Entry body */}
                <div
                  style={{
                    fontSize: 11,
                    color: "#8ab0a0",
                    lineHeight: 1.6,
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  {entry.entry}
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
