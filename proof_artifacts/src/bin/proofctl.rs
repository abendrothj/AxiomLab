use base64::{Engine as _, engine::general_purpose::STANDARD};
use proof_artifacts::cache::ProofCache;
use proof_artifacts::ci::{CiGatePolicy, evaluate_ci_gate};
use proof_artifacts::generator::{ArtifactInput, GenerateRequest, ManifestGenerator};
use proof_artifacts::manifest::{ActionPolicy, BuildIdentity, ProofManifest};
use proof_artifacts::policy::{ExecutionContext, RuntimePolicyEngine};
use proof_artifacts::signature::{SignedProofManifest, keygen, sign_manifest, verify_signed_manifest};
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
    expected_workspace_hash: Option<String>,
    expected_container_image_digest: Option<String>,
    max_manifest_age_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

fn usage() -> String {
    "Usage:\n  proofctl generate --spec <spec.json> --out <manifest.json> [--cache <cache.json>]\n  proofctl keygen --private <key.priv.b64> --public <key.pub.b64>\n  proofctl sign --manifest <manifest.json> --private-key <key.priv.b64> --out <signed.json> --key-id <id>\n  proofctl verify --signed-manifest <signed.json> --public-key <key.pub.b64>\n  proofctl gate --manifest <manifest.json> [--policy <policy.json>]\n  proofctl gate --signed-manifest <signed.json> --public-key <key.pub.b64> [--policy <policy.json>]\n  proofctl explain --manifest <manifest.json> --action <name> --git-commit <sha> --binary-hash <hash>\n  proofctl explain --signed-manifest <signed.json> --public-key <key.pub.b64> --action <name> --git-commit <sha> --binary-hash <hash>".into()
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == name).then(|| w[1].clone()))
}

fn read_manifest(path: &str) -> Result<ProofManifest, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse manifest {path}: {e}"))
}

fn read_signed_manifest(path: &str) -> Result<SignedProofManifest, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse signed manifest {path}: {e}"))
}

fn read_key_b64(path: &str) -> Result<Vec<u8>, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read key {path}: {e}"))?;
    STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("decode key {path}: {e}"))
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

            let raw = fs::read_to_string(&spec_path).unwrap_or_else(|e| {
                eprintln!("failed to read spec: {e}");
                std::process::exit(1);
            });

            let spec: GenerateRequestJson = serde_json::from_str(&raw).unwrap_or_else(|e| {
                eprintln!("failed to parse spec json: {e}");
                std::process::exit(1);
            });

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
            let manifest = ManifestGenerator::generate(&req, cache.as_mut()).unwrap_or_else(|e| {
                eprintln!("manifest generation failed: {e}");
                std::process::exit(1);
            });

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
        "keygen" => {
            let Some(priv_path) = parse_flag(rest, "--private") else {
                eprintln!("missing --private\n{}", usage());
                std::process::exit(2);
            };
            let Some(pub_path) = parse_flag(rest, "--public") else {
                eprintln!("missing --public\n{}", usage());
                std::process::exit(2);
            };
            let (sk, pk) = keygen();
            fs::write(&priv_path, format!("{}\n", STANDARD.encode(sk))).unwrap_or_else(|e| {
                eprintln!("failed to write private key: {e}");
                std::process::exit(1);
            });
            fs::write(&pub_path, format!("{}\n", STANDARD.encode(pk))).unwrap_or_else(|e| {
                eprintln!("failed to write public key: {e}");
                std::process::exit(1);
            });
            println!("generated keys: private={}, public={}", priv_path, pub_path);
        }
        "sign" => {
            let Some(manifest_path) = parse_flag(rest, "--manifest") else {
                eprintln!("missing --manifest\n{}", usage());
                std::process::exit(2);
            };
            let Some(private_key_path) = parse_flag(rest, "--private-key") else {
                eprintln!("missing --private-key\n{}", usage());
                std::process::exit(2);
            };
            let Some(out_path) = parse_flag(rest, "--out") else {
                eprintln!("missing --out\n{}", usage());
                std::process::exit(2);
            };
            let key_id = parse_flag(rest, "--key-id").unwrap_or_else(|| "default".into());

            let manifest = read_manifest(&manifest_path).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let sk = read_key_b64(&private_key_path).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let signed = sign_manifest(&manifest, &sk, &key_id).unwrap_or_else(|e| {
                eprintln!("failed to sign manifest: {e}");
                std::process::exit(1);
            });
            fs::write(&out_path, serde_json::to_string_pretty(&signed).unwrap()).unwrap_or_else(|e| {
                eprintln!("failed to write signed manifest: {e}");
                std::process::exit(1);
            });
            println!("signed manifest written to {}", out_path);
        }
        "verify" => {
            let Some(signed_path) = parse_flag(rest, "--signed-manifest") else {
                eprintln!("missing --signed-manifest\n{}", usage());
                std::process::exit(2);
            };
            let Some(public_key_path) = parse_flag(rest, "--public-key") else {
                eprintln!("missing --public-key\n{}", usage());
                std::process::exit(2);
            };
            let signed = read_signed_manifest(&signed_path).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let pk = read_key_b64(&public_key_path).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if let Err(e) = verify_signed_manifest(&signed, &pk) {
                eprintln!("verification FAILED: {e}");
                std::process::exit(1);
            }
            println!("signature verification PASSED");
        }
        "gate" => {
            let policy = if let Some(policy_path) = parse_flag(rest, "--policy") {
                let raw = fs::read_to_string(&policy_path).unwrap_or_else(|e| {
                    eprintln!("failed to read policy: {e}");
                    std::process::exit(1);
                });
                let p: CiPolicyJson = serde_json::from_str(&raw).unwrap_or_else(|e| {
                    eprintln!("failed to parse policy: {e}");
                    std::process::exit(1);
                });
                CiGatePolicy {
                    required_artifacts: p.required_artifacts,
                    require_zero_sorry: p.require_zero_sorry,
                    expected_git_commit: p.expected_git_commit,
                    expected_binary_hash: p.expected_binary_hash,
                    expected_workspace_hash: p.expected_workspace_hash,
                    expected_container_image_digest: p.expected_container_image_digest,
                    max_manifest_age_secs: p.max_manifest_age_secs,
                }
            } else {
                CiGatePolicy::default()
            };

            let manifest = if let Some(signed_path) = parse_flag(rest, "--signed-manifest") {
                let Some(public_key_path) = parse_flag(rest, "--public-key") else {
                    eprintln!("--public-key required when --signed-manifest is used");
                    std::process::exit(2);
                };
                let signed = read_signed_manifest(&signed_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                let pk = read_key_b64(&public_key_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                if let Err(e) = verify_signed_manifest(&signed, &pk) {
                    eprintln!("signature verification FAILED: {e}");
                    std::process::exit(1);
                }
                signed.manifest
            } else {
                let Some(manifest_path) = parse_flag(rest, "--manifest") else {
                    eprintln!("missing --manifest\n{}", usage());
                    std::process::exit(2);
                };
                read_manifest(&manifest_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                })
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

            let engine = if let Some(signed_path) = parse_flag(rest, "--signed-manifest") {
                let Some(public_key_path) = parse_flag(rest, "--public-key") else {
                    eprintln!("--public-key required when --signed-manifest is used");
                    std::process::exit(2);
                };
                let signed = read_signed_manifest(&signed_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                let pk = read_key_b64(&public_key_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                if let Err(e) = verify_signed_manifest(&signed, &pk) {
                    eprintln!("signature verification FAILED: {e}");
                    std::process::exit(1);
                }
                RuntimePolicyEngine::new_trusted(signed.manifest)
            } else {
                let Some(manifest_path) = parse_flag(rest, "--manifest") else {
                    eprintln!("missing --manifest\n{}", usage());
                    std::process::exit(2);
                };
                let manifest = read_manifest(&manifest_path).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
                RuntimePolicyEngine::new(manifest)
            };

            let ctx = ExecutionContext {
                git_commit,
                binary_hash,
                container_image_digest: parse_flag(rest, "--container-image-digest"),
                device_id: parse_flag(rest, "--device-id"),
                firmware_version: parse_flag(rest, "--firmware-version"),
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
