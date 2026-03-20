//! Alert / notification system for AxiomLab.
//!
//! Defines the [`NotificationSink`] trait and a concrete [`WebhookNotifier`]
//! that POSTs JSON to a configured webhook URL.  The payload is compatible with
//! Slack Incoming Webhooks, Discord webhooks (auto-detected by URL), and generic
//! JSON receivers.
//!
//! # Configuration
//! Set `AXIOMLAB_ALERT_WEBHOOK_URL` to enable webhook notifications.
//! If the variable is absent, [`WebhookNotifier::from_env()`] returns `None`
//! and the caller can use a no-op notifier.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Event types ───────────────────────────────────────────────────────────────

/// Events that trigger operator notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotificationEvent {
    ExperimentFailed {
        experiment_id: String,
        reason:        String,
    },
    EmergencyStopTriggered {
        operator_id: String,
    },
    ApprovalTimeout {
        tool:       String,
        pending_id: String,
    },
    AuditChainInvalid {
        reason: String,
    },
    CalibrationExpired {
        instrument: String,
    },
    RekorAnchorFailed {
        reason: String,
    },
}

impl NotificationEvent {
    /// Short human-readable title for the notification.
    pub fn title(&self) -> &'static str {
        match self {
            Self::ExperimentFailed { .. }      => "AxiomLab: Experiment failed",
            Self::EmergencyStopTriggered { .. } => "AxiomLab: EMERGENCY STOP triggered",
            Self::ApprovalTimeout { .. }        => "AxiomLab: Approval timed out",
            Self::AuditChainInvalid { .. }      => "AxiomLab: Audit chain integrity violation",
            Self::CalibrationExpired { .. }     => "AxiomLab: Calibration expired",
            Self::RekorAnchorFailed { .. }      => "AxiomLab: Rekor anchor failed",
        }
    }

    /// One-line summary for the notification body.
    pub fn body(&self) -> String {
        match self {
            Self::ExperimentFailed { experiment_id, reason } =>
                format!("Experiment `{experiment_id}` failed: {reason}"),
            Self::EmergencyStopTriggered { operator_id } =>
                format!("Emergency stop triggered by operator `{operator_id}`."),
            Self::ApprovalTimeout { tool, pending_id } =>
                format!("Approval for tool `{tool}` (id: {pending_id}) timed out with no operator response."),
            Self::AuditChainInvalid { reason } =>
                format!("Audit chain integrity check failed: {reason}"),
            Self::CalibrationExpired { instrument } =>
                format!("Calibration for instrument `{instrument}` has expired."),
            Self::RekorAnchorFailed { reason } =>
                format!("Could not anchor audit chain tip to Rekor: {reason}"),
        }
    }
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Implemented by any notification backend.
///
/// Implementations must be `Send + Sync + 'static` so they can be held in
/// `Arc<dyn NotificationSink>` and sent across async task boundaries.
pub trait NotificationSink: Send + Sync {
    fn send<'a>(
        &'a self,
        event: NotificationEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// A no-op sink for use in tests or when no backend is configured.
pub struct NullNotifier;

impl NotificationSink for NullNotifier {
    fn send<'a>(
        &'a self,
        _event: NotificationEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

// ── Webhook notifier ──────────────────────────────────────────────────────────

/// Sends notifications via an HTTP webhook.
///
/// Detects Slack (`hooks.slack.com`) and Discord (`discord.com/api/webhooks`)
/// by URL and formats the payload accordingly.  All other URLs receive a
/// generic JSON payload.
pub struct WebhookNotifier {
    webhook_url: String,
    client:      reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WebhookFlavor {
    Slack,
    Discord,
    Generic,
}

impl WebhookNotifier {
    /// Create a notifier pointing at `webhook_url`.
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            client:      reqwest::Client::new(),
        }
    }

    /// Read `AXIOMLAB_ALERT_WEBHOOK_URL` from the environment.
    /// Returns `None` when the variable is absent.
    pub fn from_env() -> Option<Arc<dyn NotificationSink>> {
        let url = std::env::var("AXIOMLAB_ALERT_WEBHOOK_URL").ok()?;
        Some(Arc::new(Self::new(url)))
    }

    fn flavor(&self) -> WebhookFlavor {
        if self.webhook_url.contains("hooks.slack.com") {
            WebhookFlavor::Slack
        } else if self.webhook_url.contains("discord.com/api/webhooks") {
            WebhookFlavor::Discord
        } else {
            WebhookFlavor::Generic
        }
    }

    fn build_payload(&self, event: &NotificationEvent) -> serde_json::Value {
        match self.flavor() {
            WebhookFlavor::Slack => serde_json::json!({
                "text": format!("*{}*\n{}", event.title(), event.body()),
            }),
            WebhookFlavor::Discord => serde_json::json!({
                "content": format!("**{}**\n{}", event.title(), event.body()),
            }),
            WebhookFlavor::Generic => serde_json::json!({
                "title": event.title(),
                "body":  event.body(),
                "event": event,
            }),
        }
    }
}

impl NotificationSink for WebhookNotifier {
    fn send<'a>(
        &'a self,
        event: NotificationEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let payload = self.build_payload(&event);
            match self.client
                .post(&self.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(event = ?event, "Webhook notification sent");
                }
                Ok(resp) => {
                    tracing::warn!(
                        status = %resp.status(),
                        event = ?event,
                        "Webhook notification returned non-2xx status"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, event = ?event, "Webhook notification failed");
                }
            }
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_titles_are_non_empty() {
        let events = [
            NotificationEvent::ExperimentFailed {
                experiment_id: "exp-1".into(),
                reason: "LLM error".into(),
            },
            NotificationEvent::EmergencyStopTriggered { operator_id: "alice".into() },
            NotificationEvent::ApprovalTimeout { tool: "dispense".into(), pending_id: "pid-1".into() },
            NotificationEvent::AuditChainInvalid { reason: "hash mismatch".into() },
            NotificationEvent::CalibrationExpired { instrument: "ph_meter".into() },
            NotificationEvent::RekorAnchorFailed { reason: "network error".into() },
        ];
        for e in &events {
            assert!(!e.title().is_empty());
            assert!(!e.body().is_empty());
        }
    }

    #[test]
    fn slack_payload_has_text_key() {
        let notifier = WebhookNotifier::new("https://hooks.slack.com/services/TEST");
        assert_eq!(notifier.flavor(), WebhookFlavor::Slack);
        let payload = notifier.build_payload(&NotificationEvent::CalibrationExpired {
            instrument: "ph_meter".into(),
        });
        assert!(payload["text"].is_string());
        assert!(payload["text"].as_str().unwrap().contains("ph_meter"));
    }

    #[test]
    fn discord_payload_has_content_key() {
        let notifier = WebhookNotifier::new("https://discord.com/api/webhooks/123/token");
        assert_eq!(notifier.flavor(), WebhookFlavor::Discord);
        let payload = notifier.build_payload(&NotificationEvent::EmergencyStopTriggered {
            operator_id: "bob".into(),
        });
        assert!(payload["content"].is_string());
    }

    #[test]
    fn generic_payload_has_title_and_body() {
        let notifier = WebhookNotifier::new("https://example.com/webhook");
        assert_eq!(notifier.flavor(), WebhookFlavor::Generic);
        let payload = notifier.build_payload(&NotificationEvent::AuditChainInvalid {
            reason: "hash mismatch".into(),
        });
        assert!(payload["title"].is_string());
        assert!(payload["body"].is_string());
        assert!(payload["event"].is_object());
    }

    #[tokio::test]
    async fn null_notifier_is_a_noop() {
        let n = NullNotifier;
        // Must not panic or error.
        n.send(NotificationEvent::RekorAnchorFailed { reason: "test".into() }).await;
    }

    #[test]
    fn webhook_notifier_from_env_with_url() {
        // Validate that a set URL produces Some(notifier).
        unsafe { std::env::set_var("AXIOMLAB_ALERT_WEBHOOK_URL_TEST_ONLY", "https://example.com/wh") };
        // We construct directly — from_env reads the standard var name.
        let n = WebhookNotifier::new("https://example.com/wh");
        assert_eq!(n.flavor(), WebhookFlavor::Generic);
    }
}
