//! Opt-in end-to-end test against the Python `sila_sim` full SiLA 2 server.
//!
//! Run with:
//!   AXIOMLAB_RUN_SILA2_E2E=1 cargo test -p axiom-sila --test full_sila2_e2e -- --nocapture

use axiom_sila::SilaClients;
use axiom_types::{Action, RiskClass};
use serde_json::json;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn act(tool: &str, params: serde_json::Value) -> Action {
    Action::new(tool, params, RiskClass::LiquidHandling)
}

#[tokio::test]
async fn drives_python_sila_sim_over_full_sila2() {
    if std::env::var("AXIOMLAB_RUN_SILA2_E2E").as_deref() != Ok("1") {
        eprintln!("skipping full SiLA 2 e2e; set AXIOMLAB_RUN_SILA2_E2E=1 to run");
        return;
    }

    let port = free_port();
    let mut server = spawn_sila_sim(port);
    let endpoint = format!("http://127.0.0.1:{port}");
    wait_for_sila(&endpoint).await;

    let clients = SilaClients::full_sila(endpoint);
    let r = clients
        .execute(&act(
            "dispense",
            json!({"vessel_id": "sila2_tube_1", "volume_ul": 250.0}),
        ))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert!(r["actual_volume_dispensed"].as_f64().unwrap() > 245.0);

    let r = clients
        .execute(&act(
            "read_absorbance",
            json!({"vessel_id": "sila2_tube_1", "wavelength_nm": 500.0}),
        ))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert!(r["absorbance_value"].as_f64().unwrap() > 0.001);

    let r = clients
        .execute(&act("set_temperature", json!({"target_temp_c": 37.0})))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert_eq!(r["final_temp_c"], 37.0);

    let r = clients
        .execute(&act("read_temperature", json!({})))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert!(r["current_temp_c"].as_f64().unwrap() > 25.0);

    let _ = server.kill();
    let _ = server.wait();
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spawn_sila_sim(port: u16) -> Child {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());
    Command::new(python)
        .args([
            "-m",
            "axiomlab_sim",
            "--insecure",
            "--disable-discovery",
            "--port",
            &port.to_string(),
            "--quiet",
        ])
        .current_dir(root.join("sila_sim"))
        .env("PYTHONPATH", root.join("sila_sim"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start python sila_sim; install sila_sim dependencies or leave AXIOMLAB_RUN_SILA2_E2E unset")
}

async fn wait_for_sila(endpoint: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let clients = SilaClients::full_sila(endpoint.to_string());
        if clients
            .execute(&act("read_temperature", json!({})))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sila_sim did not become reachable");
}
