use agent_runtime::approvals::{ApprovalPolicy, SignedApproval};
use proof_artifacts::manifest::RiskClass;
use proof_artifacts::policy::ExecutionContext;
use serde::Serialize;
use std::fs;

#[derive(Debug, Serialize)]
struct ApprovalVerificationReport {
    action: String,
    risk_class: String,
    passed: bool,
    approval_ids: Vec<String>,
    error: Option<String>,
}

fn usage() -> String {
    "Usage:\n  approvalctl verify --bundle <bundle.json> --action <name> --risk-class <ReadOnly|LiquidHandling|Actuation|Destructive> --git-commit <sha> --binary-hash <hash> --out <report.json>".into()
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == name).then(|| w[1].clone()))
}

fn parse_risk_class(v: &str) -> Result<RiskClass, String> {
    match v {
        "ReadOnly" => Ok(RiskClass::ReadOnly),
        "LiquidHandling" => Ok(RiskClass::LiquidHandling),
        "Actuation" => Ok(RiskClass::Actuation),
        "Destructive" => Ok(RiskClass::Destructive),
        _ => Err(format!("unknown risk class '{v}'")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("{}", usage());
        std::process::exit(2);
    }

    match args[1].as_str() {
        "verify" => {
            let Some(bundle_path) = parse_flag(&args[2..], "--bundle") else {
                eprintln!("missing --bundle\n{}", usage());
                std::process::exit(2);
            };
            let Some(action) = parse_flag(&args[2..], "--action") else {
                eprintln!("missing --action\n{}", usage());
                std::process::exit(2);
            };
            let Some(risk_class_raw) = parse_flag(&args[2..], "--risk-class") else {
                eprintln!("missing --risk-class\n{}", usage());
                std::process::exit(2);
            };
            let Some(git_commit) = parse_flag(&args[2..], "--git-commit") else {
                eprintln!("missing --git-commit\n{}", usage());
                std::process::exit(2);
            };
            let Some(binary_hash) = parse_flag(&args[2..], "--binary-hash") else {
                eprintln!("missing --binary-hash\n{}", usage());
                std::process::exit(2);
            };
            let Some(out_path) = parse_flag(&args[2..], "--out") else {
                eprintln!("missing --out\n{}", usage());
                std::process::exit(2);
            };

            let risk_class = parse_risk_class(&risk_class_raw).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2);
            });

            let raw = fs::read_to_string(&bundle_path).unwrap_or_else(|e| {
                eprintln!("failed to read bundle {bundle_path}: {e}");
                std::process::exit(1);
            });
            let bundle: Vec<SignedApproval> = serde_json::from_str(&raw).unwrap_or_else(|e| {
                eprintln!("failed to parse bundle {bundle_path}: {e}");
                std::process::exit(1);
            });

            let ctx = ExecutionContext {
                git_commit,
                binary_hash,
                container_image_digest: None,
                device_id: None,
                firmware_version: None,
            };

            let params = serde_json::json!({ "approval_bundle": bundle });
            let policy = ApprovalPolicy::default_high_risk();
            // Pass session_nonce=None: approvalctl verifies bundles out-of-band
            // without a live session nonce.
            let result = policy.validate_action(&action, Some(risk_class.clone()), &ctx, &params, None);

            let report = match result {
                Ok(ids) => ApprovalVerificationReport {
                    action,
                    risk_class: format!("{:?}", risk_class),
                    passed: true,
                    approval_ids: ids,
                    error: None,
                },
                Err(e) => ApprovalVerificationReport {
                    action,
                    risk_class: format!("{:?}", risk_class),
                    passed: false,
                    approval_ids: Vec::new(),
                    error: Some(e),
                },
            };

            if let Some(parent) = std::path::Path::new(&out_path).parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("failed to create output dir: {e}");
                    std::process::exit(1);
                }
            }

            if let Err(e) = fs::write(&out_path, serde_json::to_string_pretty(&report).unwrap()) {
                eprintln!("failed to write report: {e}");
                std::process::exit(1);
            }

            if report.passed {
                println!("approval verification PASSED");
            } else {
                eprintln!(
                    "approval verification FAILED: {}",
                    report.error.as_deref().unwrap_or("unknown error")
                );
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("unknown command\n{}", usage());
            std::process::exit(2);
        }
    }
}
