//! ELN / LIMS adapter — exports studies to electronic lab notebooks.
//!
//! Defines the [`ELNAdapter`] trait and a concrete [`BenchlingAdapter`] that
//! maps AxiomLab study records to Benchling notebook entries via their REST API.
//!
//! # Configuration
//! Set the following env vars to enable Benchling export:
//! ```text
//! AXIOMLAB_BENCHLING_TOKEN       # API token (Bearer)
//! AXIOMLAB_BENCHLING_TENANT      # e.g. "myorg.benchling.com"
//! AXIOMLAB_BENCHLING_PROJECT_ID  # Project id where entries are created
//! ```
//! When these vars are absent, `BenchlingAdapter::from_env()` returns `None`
//! and `POST /api/export/benchling/<study_id>` returns 503.

use crate::discovery::{RunSummary, StudyRecord};
use reqwest::Client;
use serde::{Deserialize, Serialize};

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Implemented by each ELN backend.
pub trait ELNAdapter: Send + Sync {
    /// Export a study and its associated runs to the ELN.
    ///
    /// Returns a URL to the created entry on success.
    fn export_study<'a>(
        &'a self,
        study: &'a StudyRecord,
        runs: &'a [RunSummary],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

// ── Benchling adapter ─────────────────────────────────────────────────────────

/// Exports AxiomLab study records to Benchling notebook entries.
pub struct BenchlingAdapter {
    tenant:     String,
    project_id: String,
    client:     Client,
}

impl BenchlingAdapter {
    /// Create an adapter with the given credentials.
    pub fn new(api_token: &str, tenant: impl Into<String>, project_id: impl Into<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        let client = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build Benchling HTTP client");
        Self {
            tenant:     tenant.into(),
            project_id: project_id.into(),
            client,
        }
    }

    /// Read credentials from environment variables.
    ///
    /// Returns `None` when any required variable is absent.
    pub fn from_env() -> Option<Self> {
        let token      = std::env::var("AXIOMLAB_BENCHLING_TOKEN").ok()?;
        let tenant     = std::env::var("AXIOMLAB_BENCHLING_TENANT").ok()?;
        let project_id = std::env::var("AXIOMLAB_BENCHLING_PROJECT_ID").ok()?;
        Some(Self::new(&token, tenant, project_id))
    }

    fn base_url(&self) -> String {
        format!("https://{}/api/v2", self.tenant)
    }
}

// ── Benchling API response shapes (subset) ────────────────────────────────────

#[derive(Deserialize)]
struct BenchlingEntry {
    id:        String,
    #[serde(rename = "webURL")]
    web_url:   Option<String>,
}

#[derive(Serialize)]
struct CreateEntryRequest<'a> {
    name:        String,
    #[serde(rename = "folderId", skip_serializing_if = "Option::is_none")]
    folder_id:   Option<&'a str>,
    #[serde(rename = "projectId")]
    project_id:  &'a str,
    fields:      serde_json::Value,
    day:         String,
    author_ids:  Vec<String>,
    entry_template_id: Option<String>,
}

#[derive(Serialize)]
struct CreateEntryWrapper<'a> {
    entry: CreateEntryRequest<'a>,
}

impl ELNAdapter for BenchlingAdapter {
    fn export_study<'a>(
        &'a self,
        study: &'a StudyRecord,
        runs: &'a [RunSummary],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            // Build a rich text description of the study and runs.
            let mut body_text = format!(
                "**Study:** {}\n**Director:** {}\n**Status:** {}\n\n",
                study.title,
                study.study_director_id,
                study.status,
            );

            if !study.protocol_ids.is_empty() {
                body_text.push_str(&format!(
                    "**Pre-registered protocols:** {}\n",
                    study.protocol_ids.join(", ")
                ));
            }

            if !study.run_ids.is_empty() {
                body_text.push_str(&format!(
                    "**Completed runs:** {}\n\n",
                    study.run_ids.join(", ")
                ));
            }

            if !runs.is_empty() {
                body_text.push_str("## Runs\n");
                for run in runs {
                    body_text.push_str(&format!(
                        "- **{}** ({}/{} steps): {}\n",
                        run.protocol_name,
                        run.steps_succeeded,
                        run.steps_total,
                        run.conclusion,
                    ));
                }
            }

            if let (Some(rev), Some(hash)) = (&study.qa_reviewer_id, &study.qa_sign_off_hash) {
                body_text.push_str(&format!(
                    "\n**QA Reviewer:** {rev}\n**Sign-off hash:** `{hash}`\n"
                ));
            }

            let today = chrono_today();
            let req_body = CreateEntryWrapper {
                entry: CreateEntryRequest {
                    name:               format!("[AxiomLab] {}", study.title),
                    folder_id:          None,
                    project_id:         &self.project_id,
                    fields:             serde_json::json!({ "Body": body_text }),
                    day:                today,
                    author_ids:         Vec::new(),
                    entry_template_id:  None,
                },
            };

            let url = format!("{}/entries", self.base_url());
            let resp = self.client
                .post(&url)
                .json(&req_body)
                .send()
                .await
                .map_err(|e| format!("Benchling request failed: {e}"))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body   = resp.text().await.unwrap_or_default();
                return Err(format!("Benchling returned {status}: {body}"));
            }

            let entry: BenchlingEntry = resp.json().await
                .map_err(|e| format!("Benchling response parse error: {e}"))?;

            Ok(entry.web_url.unwrap_or_else(|| {
                format!("https://{}/entries/{}", self.tenant, entry.id)
            }))
        })
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Returns today's date as "YYYY-MM-DD" (UTC).
fn chrono_today() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    date_from_epoch_secs(secs)
}

/// Convert Unix epoch seconds to "YYYY-MM-DD" (UTC, proleptic Gregorian).
///
/// Extracted so it can be tested with known timestamps without mocking the clock.
fn date_from_epoch_secs(secs: u64) -> String {
    let days = secs / 86400;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let m   = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Known epoch → date mappings (UTC):
    //   0            = 1970-01-01 (Unix epoch itself)
    //   86400        = 1970-01-02 (exactly one day later)
    //   946684800    = 2000-01-01 (Y2K midnight)
    //   951868800    = 2000-03-01 (day after 2000-02-29 — confirms 2000 treated as leap year)
    //   1704067200   = 2024-01-01

    #[test]
    fn date_algorithm_unix_epoch() {
        assert_eq!(date_from_epoch_secs(0), "1970-01-01");
    }

    #[test]
    fn date_algorithm_one_day_later() {
        assert_eq!(date_from_epoch_secs(86400), "1970-01-02");
    }

    #[test]
    fn date_algorithm_y2k() {
        assert_eq!(date_from_epoch_secs(946684800), "2000-01-01");
    }

    #[test]
    fn date_algorithm_year_2000_is_leap_year() {
        // 2000-02-29 exists (year 2000 is divisible by 400).
        // 2000-03-01 = 946684800 + 31*86400 (January) + 29*86400 (February)
        let mar_1_2000 = 946684800_u64 + 31 * 86400 + 29 * 86400;
        assert_eq!(date_from_epoch_secs(mar_1_2000), "2000-03-01");
    }

    #[test]
    fn date_algorithm_2024_jan_1() {
        assert_eq!(date_from_epoch_secs(1704067200), "2024-01-01");
    }

    #[test]
    fn chrono_today_format_and_plausible_year() {
        let date = chrono_today();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        let year: u32 = date[..4].parse().unwrap();
        assert!(year >= 2024, "clock returned year {year} — suspiciously old");
    }

    #[test]
    fn base_url_uses_tenant() {
        let adapter = BenchlingAdapter::new("tok", "myorg.benchling.com", "proj_x");
        assert_eq!(adapter.base_url(), "https://myorg.benchling.com/api/v2");
    }

    #[test]
    fn base_url_does_not_double_slash() {
        // Tenant must not have a trailing slash — verify the URL is well-formed.
        let adapter = BenchlingAdapter::new("tok", "lab.example.com", "p");
        let url = adapter.base_url();
        assert!(!url.contains("//api"), "double slash in URL: {url}");
        assert!(url.ends_with("/api/v2"), "unexpected suffix: {url}");
    }
}
