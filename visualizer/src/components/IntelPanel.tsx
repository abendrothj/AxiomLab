import { useState } from "react";
import { NotebookEntryEvent, StateTransitionEvent, ToolExecutionEvent } from "../types";
import LabNotebook from "./LabNotebook";
import BlueprintGraph from "./BlueprintGraph";
import ActivityFeed from "./ActivityFeed";

type Tab = "discoveries" | "blueprint" | "activity";

interface Props {
  open: boolean;
  onClose: () => void;
  notebook: NotebookEntryEvent[];
  transitions: StateTransitionEvent[];
  toolEvents: ToolExecutionEvent[];
}

const TABS: { id: Tab; label: string; glyph: string }[] = [
  { id: "discoveries", label: "Discoveries", glyph: "◉" },
  { id: "blueprint",   label: "Blueprint",   glyph: "⬡" },
  { id: "activity",    label: "Activity",    glyph: "⚡" },
];

export default function IntelPanel({ open, onClose, notebook, transitions, toolEvents }: Props) {
  const [tab, setTab] = useState<Tab>("discoveries");

  return (
    <>
      {/* Backdrop — clicking closes the panel */}
      {open && (
        <div
          onClick={onClose}
          style={{
            position: "absolute",
            inset: 0,
            background: "rgba(0,0,0,0.25)",
            backdropFilter: "blur(1px)",
            zIndex: 10,
          }}
        />
      )}

      {/* Slide-in panel */}
      <div
        style={{
          position: "absolute",
          top: 0,
          right: 0,
          bottom: 0,
          width: 420,
          transform: open ? "translateX(0)" : "translateX(100%)",
          transition: "transform 0.28s cubic-bezier(0.4,0,0.2,1)",
          zIndex: 20,
          display: "flex",
          flexDirection: "column",
          background: "rgba(10,12,18,0.94)",
          backdropFilter: "blur(16px)",
          borderLeft: "1px solid #1a2a3a",
        }}
      >
        {/* Panel header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            padding: "0 4px 0 16px",
            height: 48,
            borderBottom: "1px solid #141e2a",
            flexShrink: 0,
            gap: 4,
          }}
        >
          {TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              style={{
                flex: 1,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                gap: 6,
                padding: "6px 0",
                background: tab === t.id ? "#0d1a28" : "transparent",
                border: "none",
                borderBottom: `2px solid ${tab === t.id ? "#00ff9d" : "transparent"}`,
                borderRadius: "2px 2px 0 0",
                color: tab === t.id ? "#00ff9d" : "#3a5a6a",
                fontFamily: '"JetBrains Mono", monospace',
                fontSize: 10,
                fontWeight: 700,
                letterSpacing: "0.1em",
                cursor: "pointer",
              }}
            >
              <span style={{ fontSize: 12 }}>{t.glyph}</span>
              {t.label.toUpperCase()}
            </button>
          ))}

          <button
            onClick={onClose}
            style={{
              marginLeft: 4,
              width: 28,
              height: 28,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              background: "transparent",
              border: "1px solid #1a2a3a",
              borderRadius: 3,
              color: "#3a5a6a",
              cursor: "pointer",
              fontSize: 14,
              flexShrink: 0,
            }}
          >
            ×
          </button>
        </div>

        {/* Panel body */}
        <div style={{ flex: 1, overflow: "hidden", minHeight: 0 }}>
          {tab === "discoveries" && <LabNotebook entries={notebook} />}
          {tab === "blueprint"   && <BlueprintGraph transitions={transitions} />}
          {tab === "activity"    && <ActivityFeed events={toolEvents} />}
        </div>
      </div>
    </>
  );
}
