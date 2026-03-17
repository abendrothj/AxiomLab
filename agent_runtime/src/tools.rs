//! Tool dispatch system for the agent runtime.
//!
//! Every action the agent can perform in the physical lab is modelled
//! as a [`Tool`].  The [`ToolRegistry`] holds the set of tools the
//! current sandbox permits and dispatches calls by name.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

// ── Error ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    NotFound(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("invalid parameters: {0}")]
    BadParams(String),
}

// ── Types ────────────────────────────────────────────────────────

/// JSON-in / JSON-out tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    pub output: serde_json::Value,
    pub success: bool,
}

/// A boxed async handler: `params → Result<output, error_msg>`.
pub type ToolHandler = Box<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>
        + Send
        + Sync,
>;

/// Description of a tool for inclusion in LLM prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

// ── Registry ─────────────────────────────────────────────────────

/// Holds the set of tools available to the agent and dispatches calls.
pub struct ToolRegistry {
    handlers: HashMap<String, ToolHandler>,
    specs: Vec<ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            specs: Vec::new(),
        }
    }

    /// Register a tool with its spec and handler.
    pub fn register(&mut self, spec: ToolSpec, handler: ToolHandler) {
        self.handlers.insert(spec.name.clone(), handler);
        self.specs.push(spec);
    }

    /// Return the specs of all registered tools (for the LLM system prompt).
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// Dispatch a tool call.
    pub async fn dispatch(&self, call: &ToolCall) -> ToolResult {
        match self.handlers.get(&call.name) {
            Some(handler) => match handler(call.params.clone()).await {
                Ok(output) => ToolResult {
                    name: call.name.clone(),
                    output,
                    success: true,
                },
                Err(e) => ToolResult {
                    name: call.name.clone(),
                    output: serde_json::Value::String(e),
                    success: false,
                },
            },
            None => ToolResult {
                name: call.name.clone(),
                output: serde_json::Value::String(format!("tool not found: {}", call.name)),
                success: false,
            },
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Built-in tools ───────────────────────────────────────────────

/// Register the default lab-hardware tools.
pub fn register_lab_tools(registry: &mut ToolRegistry) {
    // ── move_arm ──
    registry.register(
        ToolSpec {
            name: "move_arm".into(),
            description: "Move the robotic arm to (x, y, z) in mm.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "z": { "type": "number" }
                },
                "required": ["x", "y", "z"]
            }),
        },
        Box::new(|params| {
            Box::pin(async move {
                let x = params["x"].as_f64().ok_or("missing x")?;
                let y = params["y"].as_f64().ok_or("missing y")?;
                let z = params["z"].as_f64().ok_or("missing z")?;
                tracing::info!(x, y, z, "moving arm");
                Ok(serde_json::json!({ "status": "moved", "x": x, "y": y, "z": z }))
            })
        }),
    );

    // ── read_sensor ──
    registry.register(
        ToolSpec {
            name: "read_sensor".into(),
            description: "Read a named sensor and return its current value.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "sensor_id": { "type": "string" }
                },
                "required": ["sensor_id"]
            }),
        },
        Box::new(|params| {
            Box::pin(async move {
                let id = params["sensor_id"]
                    .as_str()
                    .ok_or("missing sensor_id")?
                    .to_owned();
                tracing::info!(sensor_id = %id, "reading sensor");
                // SECURITY: Hardware stub for development.
                // Production: inject SensorDriver trait. See OPERATOR_GUIDE.md section 2.3.
                #[cfg(not(feature = "hardware"))]
                let value = 7.04_f64;
                #[cfg(feature = "hardware")]
                let value: f64 = return Err(
                    "sensor_driver not injected: build with hardware feature \
                     and provide a SensorDriver implementation. \
                     See OPERATOR_GUIDE.md section 2.3.".into()
                );
                Ok(serde_json::json!({ "sensor_id": id, "value": value, "unit": "pH", "source": "STUB" }))
            })
        }),
    );

    // ── dispense ──
    registry.register(
        ToolSpec {
            name: "dispense".into(),
            description: "Dispense a volume of liquid from a specified pump.".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pump_id": { "type": "string" },
                    "volume_ul": { "type": "number" }
                },
                "required": ["pump_id", "volume_ul"]
            }),
        },
        Box::new(|params| {
            Box::pin(async move {
                let pump = params["pump_id"]
                    .as_str()
                    .ok_or("missing pump_id")?
                    .to_owned();
                let vol = params["volume_ul"].as_f64().ok_or("missing volume_ul")?;
                tracing::info!(pump_id = %pump, volume_ul = vol, "dispensing");
                Ok(serde_json::json!({ "pump_id": pump, "dispensed_ul": vol }))
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatch_known_tool() {
        let mut reg = ToolRegistry::new();
        register_lab_tools(&mut reg);
        let call = ToolCall {
            name: "read_sensor".into(),
            params: serde_json::json!({ "sensor_id": "pH-1" }),
        };
        let res = reg.dispatch(&call).await;
        assert!(res.success);
        assert_eq!(res.output["sensor_id"], "pH-1");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool() {
        let reg = ToolRegistry::new();
        let call = ToolCall {
            name: "nuke_lab".into(),
            params: serde_json::json!({}),
        };
        let res = reg.dispatch(&call).await;
        assert!(!res.success);
    }

    #[test]
    fn specs_available_after_registration() {
        let mut reg = ToolRegistry::new();
        register_lab_tools(&mut reg);
        assert_eq!(reg.specs().len(), 3);
    }
}
