use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub unix_secs: u64,
    pub trace_id: String,
    pub action: String,
    pub decision: String,
    pub reason: String,
    pub success: bool,
}

pub fn emit_jsonl(path: &str, event: &AuditEvent) -> Result<(), std::io::Error> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(event)
        .map_err(|e| std::io::Error::other(format!("serialize audit event: {e}")))?;
    writeln!(f, "{}", line)?;
    Ok(())
}

pub fn trace_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", prefix, nanos)
}
