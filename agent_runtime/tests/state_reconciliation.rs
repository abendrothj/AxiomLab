//! Integration test: vessel-state reconciliation after a simulated network timeout.
//!
//! # Scenario
//!
//! The Python SiLA2 mock commits `registry.dispense()` **before** returning the
//! gRPC response.  If the TCP connection drops during the sleep that follows the
//! commit, the Rust client sees an error and does not update `LabState`.
//!
//! This test models that phantom-commit path:
//!
//!  1.  A minimal HTTP server stands in for the Python mock's vessel-state
//!      endpoint.  It serves one "phase":
//!        - **timeout phase** — the connection is accepted then immediately
//!          dropped (simulates the network dying mid-response).
//!        - **recover phase** — the endpoint returns correct JSON showing the
//!          committed volume the Rust side never recorded.
//!
//!  2.  `reconcile_vessel_state()` is called against a fresh `LabState` and the
//!      recovered volume snapshot.
//!
//!  3.  We assert that the phantom commit is detected and the audit trail would
//!      record a desync event.
//!
//!  4.  `apply_reconciliation()` is called and we verify `LabState` is updated.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_runtime::hardware::VesselVolume;
use agent_runtime::lab_state::LabState;
use agent_runtime::reconciler::{
    apply_reconciliation, reconcile_vessel_state, TOLERANCE_UL,
};

// ── Minimal HTTP stub ─────────────────────────────────────────────────────────

/// Phase the stub server is in.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// Drop the connection immediately after accept (simulates timeout).
    Timeout,
    /// Serve the JSON body normally.
    Recover,
}

/// Spawn a TCP listener that:
///   - In `Phase::Timeout` — drops the connection right away.
///   - In `Phase::Recover` — writes a minimal HTTP/1.1 200 response with `body`.
///
/// Returns the bound address.
fn spawn_stub_server(phase: Arc<Mutex<Phase>>, body: String) -> SocketAddr {
    use std::io::Write;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let p = *phase.lock().unwrap();
            match p {
                Phase::Timeout => {
                    // Accept then immediately drop — simulates the connection dying.
                    drop(stream);
                }
                Phase::Recover => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\
                         \r\n\
                         {}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    // Only serve one recovery response, then keep accepting (loop).
                }
            }
        }
    });

    addr
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn vessel_volume(volume_ul: f64) -> VesselVolume {
    VesselVolume { volume_ul, max_volume_ul: 50_000.0 }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Core reconciliation logic is pure (no I/O).
/// This test exercises the algorithm directly without network.
#[test]
fn phantom_commit_detected_and_applied() {
    let lab = LabState::default();

    // Instrument reports 500 µL in beaker_A — Rust never recorded it (phantom commit).
    let mut hw: HashMap<String, VesselVolume> = HashMap::new();
    hw.insert("beaker_A".into(), vessel_volume(500.0));

    let desyncs = reconcile_vessel_state(&lab, &hw);

    assert_eq!(desyncs.len(), 1, "expected exactly one desync");
    let d = &desyncs[0];
    assert_eq!(d.vessel_id, "beaker_A");
    assert_eq!(d.expected_ul, None, "vessel unknown to LabState → expected_ul = None");
    assert!((d.actual_ul - 500.0).abs() < f64::EPSILON);
    assert!((d.delta_ul - 500.0).abs() < f64::EPSILON);
    assert!(d.delta_ul > TOLERANCE_UL, "delta must exceed tolerance to be reported");

    // Now apply the reconciliation and check LabState is updated.
    let mut lab = lab;
    apply_reconciliation(&mut lab, &desyncs);

    let contents = lab
        .vessel_contents
        .get("beaker_A")
        .expect("beaker_A must now be in vessel_contents");
    assert!(
        contents.contains(&"__phantom__".to_string()),
        "phantom marker must be recorded"
    );
}

/// Multiple vessels can desync in a single reconciliation pass.
#[test]
fn multiple_vessel_desyncs() {
    let lab = LabState::default();

    let mut hw = HashMap::new();
    hw.insert("beaker_A".into(), vessel_volume(500.0));
    hw.insert("tube_1".into(), vessel_volume(200.0));
    hw.insert("reservoir".into(), vessel_volume(0.0)); // zero → within tolerance

    let desyncs = reconcile_vessel_state(&lab, &hw);

    assert_eq!(desyncs.len(), 2, "reservoir (0 µL) must not be flagged");

    let vessel_ids: Vec<&str> = desyncs.iter().map(|d| d.vessel_id.as_str()).collect();
    assert!(vessel_ids.contains(&"beaker_A"));
    assert!(vessel_ids.contains(&"tube_1"));
}

/// Volumes within TOLERANCE_UL are not flagged (covers pump noise).
#[test]
fn small_delta_not_flagged() {
    let lab = LabState::default();

    let mut hw = HashMap::new();
    hw.insert("beaker_A".into(), vessel_volume(TOLERANCE_UL - 0.1));

    let desyncs = reconcile_vessel_state(&lab, &hw);
    assert!(desyncs.is_empty(), "delta within tolerance must not desync");
}

/// A vessel the Rust side has recorded contents for, with a non-zero instrument
/// volume, should be considered in-sync (instrument is ground truth).
#[test]
fn recorded_vessel_with_liquid_is_in_sync() {
    use agent_runtime::lab_state::Reagent;

    let mut lab = LabState::default();
    lab.register_reagent(Reagent {
        id: "r1".into(),
        name: "HCl".into(),
        cas_number: None,
        lot_number: "L001".into(),
        concentration: None,
        concentration_unit: None,
        volume_ul: 500.0,
        expiry_secs: None,
        ghs_hazard_codes: vec![],
        reference_material_id: None,
        nominal_ph: Some(1.0),
    });
    lab.add_to_vessel("beaker_A", "r1");

    let mut hw = HashMap::new();
    hw.insert("beaker_A".into(), vessel_volume(500.0));

    let desyncs = reconcile_vessel_state(&lab, &hw);
    assert!(desyncs.is_empty(), "known vessel with matching non-zero volume must be in sync");
}

/// Simulates the full phantom-commit lifecycle over a real TCP connection:
///  1. HTTP stub drops the connection (simulates gRPC timeout → Rust sees error, LabState not updated).
///  2. HTTP stub serves recovered state (Python committed 300 µL, Rust mental model has 0).
///  3. reconciler detects the desync.
///  4. apply_reconciliation patches LabState.
#[tokio::test]
async fn network_timeout_then_reconcile() {
    let phase = Arc::new(Mutex::new(Phase::Timeout));
    let body = r#"{"beaker_A":{"volume_ul":300.0,"max_volume_ul":50000.0}}"#.to_string();
    let addr = spawn_stub_server(Arc::clone(&phase), body.clone());

    let url = format!("http://{}/vessel_state", addr);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();

    // ── Phase 1: timeout ──────────────────────────────────────────────────────
    // The server drops the connection immediately.  The client should see an
    // error — equivalent to a gRPC timeout mid-flight after the Python side
    // already committed the dispense.
    let timeout_result = client.get(&url).send().await;
    assert!(
        timeout_result.is_err(),
        "dropped connection must produce an error (got Ok instead)"
    );

    // ── Phase 2: recovery ─────────────────────────────────────────────────────
    // Switch the stub to serving normal JSON.  This models the next call to
    // query_vessel_volumes() after the transient failure is resolved.
    *phase.lock().unwrap() = Phase::Recover;

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("recovery GET must succeed");
    assert!(resp.status().is_success());

    let hw: HashMap<String, VesselVolume> = resp
        .json()
        .await
        .expect("recovery JSON must parse");

    // ── Phase 3: reconcile ────────────────────────────────────────────────────
    let lab = LabState::default(); // Rust never recorded the dispense
    let desyncs = reconcile_vessel_state(&lab, &hw);

    assert_eq!(
        desyncs.len(),
        1,
        "exactly one phantom commit must be detected"
    );
    let d = &desyncs[0];
    assert_eq!(d.vessel_id, "beaker_A");
    assert!((d.actual_ul - 300.0).abs() < f64::EPSILON);
    assert_eq!(d.expected_ul, None);

    // ── Phase 4: apply reconciliation ─────────────────────────────────────────
    let mut lab = lab;
    apply_reconciliation(&mut lab, &desyncs);

    assert!(
        lab.vessel_contents
            .get("beaker_A")
            .map(|v| v.contains(&"__phantom__".to_string()))
            .unwrap_or(false),
        "LabState must record phantom contents after reconciliation"
    );
}
