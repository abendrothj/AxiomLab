// ── WebSocket event payloads ──────────────────────────────────────
//
// Field names are exact snake_case matches to the Rust serde-derived structs
// in agent_runtime/src/events.rs.  Do not rename without updating both sides.

export interface LlmTokenEvent {
  token: string;
}

export interface StateTransitionEvent {
  from: string;
  to: string;
  experiment_id: string;
  timestamp_ms: number;
}

export interface ToolExecutionEvent {
  tool: string;
  target: string;
  params: Record<string, unknown>;
  max_safe_limit: number;
  status: "success" | "rejected";
  reason: string;
}

export interface NotebookEntryEvent {
  experiment_id: string;
  entry: string;
  timestamp_ms: number;
  tool_that_triggered: string;
  outcome: "discovery" | "rejection" | "inconclusive";
}

// ── Typed param shapes ────────────────────────────────────────────

export interface DispenseParams {
  pump_id: string;
  volume_ul: number;
}

export interface MoveArmParams {
  x: number;
  y: number;
  z: number;
}

export interface AbsorbanceParams {
  vessel_id: string;
  wavelength_nm: number;
}

export interface TemperatureParams {
  vessel_id: string;
  target_mk: number;
}

export interface StirParams {
  vessel_id: string;
  rpm: number;
}

// ── Event name constants ──────────────────────────────────────────

export const EVENTS = {
  LLM_TOKEN: "llm_token",
  STATE_TRANSITION: "state_transition",
  TOOL_EXECUTION: "tool_execution",
  NOTEBOOK_ENTRY: "notebook_entry",
} as const;

// ── Stage display helpers ─────────────────────────────────────────

export type Stage =
  | "Proposed"
  | "CodeGenerated"
  | "Verified"
  | "Executing"
  | "Analysing"
  | "Completed"
  | "Failed"
  | "";

export const STAGE_COLORS: Record<string, string> = {
  Proposed: "#6c757d",
  CodeGenerated: "#0d6efd",
  Verified: "#20c997",
  Executing: "#fd7e14",
  Analysing: "#6f42c1",
  Completed: "#00ff9d",
  Failed: "#ff3b3b",
  "": "#3a4a5a",
};
