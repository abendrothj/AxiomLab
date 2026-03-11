use proof_artifacts::cache::ProofCache;
use proof_artifacts::ci::{CiGatePolicy, evaluate_ci_gate};
use proof_artifacts::generator::{ArtifactInput, GenerateRequest, ManifestGenerator};
use proof_artifacts::manifest::{ActionPolicy, BuildIdentity, ProofManifest};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct ArtifactInputJson {
    id: String,
    source_path: String,
    mir_path: Option<String>,
    lean_paths: Vec<String>,
    verus_proof_path: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct GenerateRequestJson {
    build: BuildIdentity,
    artifacts: Vec<ArtifactInputJson>,
    #[serde(default)]
    actions: Vec<ActionPolicy>,
}

#[derive(Debug, Deserialize)]
struct CiPolicyJson {
    #[serde(default)]
    required_artifacts: Vec<String>,
    #[serde(default = "default_true")]
    require_zero_sorry: bool,
    expected_git_commit: Option<String>,
    expected_binary_hash: Option<String>,
}

fn default_true() -> bool {
    true
}

fn usage() -> String {
    "Usage:\n  proofctl generate --spec <spec.json> --out <manifest.json> [--cache <cache.json>]\n  proofctl gate --manifest <manifest.json> [--policy <policy.json>]\n  proofctl explain --manifest <manifest.json> --action <name> --git-commit <sha> --binary-hash <hash>".into()
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == name).then(|| w[1].clone()))
}

fn read_manifest(path: &str) -> Result<ProofManifest, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse manifest {path}: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("{}", usage());
        std::process::exit(2);
    }

    let cmd = args[1].as_str();
    let rest = &args[2..];

    match cmd {
        "generate" => {
            let Some(spec_path) = parse_flag(rest, "--spec") else {
                eprintln!("missing --spec\n{}", usage());
                std::process::exit(2);
            };
            let Some(out_path) = parse_flag(rest, "--out") else {
                eprintln!("missing --out\n{}", usage());
                std::process::exit(2);
            };
            let cache_path = parse_flag(rest, "--cache");

            let raw = match fs::read_to_string(&spec_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to read spec: {e}");
                    std::process::exit(1);
                }
            };

            let spec: GenerateRequestJson = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("failed to parse spec json: {e}");
                    std::process::exit(1);
                }
            };

            let req = GenerateRequest {
                build: spec.build,
                artifacts: spec
                    .artifacts
                    .into_iter()
                    .map(|a| ArtifactInput {
                        id: a.id,
                        source_path: PathBuf::from(a.source_path),
                        mir_path: a.mir_path.map(PathBuf::from),
                        lean_paths: a.lean_paths.into_iter().map(PathBuf::from).collect(),
                        verus_proof_path: a.verus_proof_path.map(PathBuf::from),
                        metadata: a.metadata,
                    })
                    .collect(),
                actions: spec.actions,
            };

            let mut cache = cache_path
                .as_ref()
                .and_then(|p| ProofCache::load(PathBuf::from(p).as_path()).ok());
            let manifest = match ManifestGenerator::generate(&req, cache.as_mut()) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("manifest generation failed: {e}");
                    std::process::exit(1);
                }
            };

            if let Some(parent) = PathBuf::from(&out_path).parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    eprintln!("failed to create output directory: {e}");
                    std::process::exit(1);
                }
            }
            if let Err(e) = fs::write(&out_path, serde_json::to_string_pretty(&manifest).unwrap()) {
                eprintln!("failed to write manifest: {e}");
                std::process::exit(1);
            }

            if let (Some(path), Some(cache_obj)) = (cache_path, cache) {
                if let Err(e) = cache_obj.save(PathBuf::from(path).as_path()) {
                    eprintln!("failed to save cache: {e}");
                    std::process::exit(1);
                }
            }

            println!("manifest written to {}", out_path);
        }
        "gate" => {
            let Some(manifest_path) = parse_flag(rest, "--manifest") else {
                eprintln!("missing --manifest\n{}", usage());
                std::process::exit(2);
            };
            let manifest = match read_manifest(&manifest_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };

            let policy = if let Some(policy_path) = parse_flag(rest, "--policy") {
                let raw = match fs::read_to_string(&policy_path) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("failed to read policy: {e}");
                        std::process::exit(1);
                    }
                };
                let p: CiPolicyJson = match serde_json::from_str(&raw) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("failed to parse policy: {e}");
                        std::process::exit(1);
                    }
                };
                CiGatePolicy {
                    required_artifacts: p.required_artifacts,
                    require_zero_sorry: p.require_zero_sorry,
                    expected_git_commit: p.expected_git_commit,
                    expected_binary_hash: p.expected_binary_hash,
                }
            } else {
                CiGatePolicy::default()
            };

            let report = evaluate_ci_gate(&manifest, &policy);
            if report.passed {
                println!("CI gate PASSED");
            } else {
                println!("CI gate FAILED");
                for v in report.violations {
                    println!("- {v}");
                }
                std::process::exit(1);
            }
        }
        "explain" => {
            let Some(manifest_path) = parse_flag(rest, "--manifest") else {
                eprintln!("missing --manifest\n{}", usage());
                std::process::exit(2);
            };
            let Some(action) = parse_flag(rest, "--action") else {
                eprintln!("missing --action\n{}", usage());
                std::process::exit(2);
            };
            let Some(git_commit) = parse_flag(rest, "--git-commit") else {
                eprintln!("missing --git-commit\n{}", usage());
                std::process::exit(2);
            };
            let Some(binary_hash) = parse_flag(rest, "--binary-hash") else {
                eprintln!("missing --binary-hash\n{}", usage());
                std::process::exit(2);
            };

            let manifest = match read_manifest(&manifest_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };
            let engine = RuntimePolicyEngine::new(manifest);
            let ctx = ExecutionContext {
                git_commit,
                binary_hash,
            };

            match engine.authorize(&action, &ctx) {
                Ok(()) => println!("ALLOW: action is authorized"),
                Err(e) => println!("DENY: {e}"),
            }

            let report = engine.explain(&action);
            println!("decision: {:?}", report.decision);
            println!("reason: {}", report.reason);
            if let Some(policy) = report.matched_policy {
                println!("policy: {policy}");
            }
            if !report.artifacts_checked.is_empty() {
                println!("artifacts:");
                for (id, status, sorry) in report.artifacts_checked {
                    println!("- {id}: {:?}, sorry={sorry}", status);
                }
            }
        }
        _ => {
            eprintln!("unknown command: {cmd}\n{}", usage());
            std::process::exit(2);
        }
    }
}
