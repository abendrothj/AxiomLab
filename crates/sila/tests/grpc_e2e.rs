//! End-to-end gRPC: drive `SilaClients::grpc` against the in-process mock
//! instrument server over a real TCP/HTTP2 connection.

use axiom_sila::{SilaClients, spawn_mock_server};
use axiom_types::{Action, RiskClass};
use serde_json::json;

fn act(tool: &str, params: serde_json::Value) -> Action {
    Action::new(tool, params, RiskClass::LiquidHandling)
}

#[tokio::test]
async fn dispense_read_temperature_round_trip_over_grpc() {
    let (endpoint, server) = spawn_mock_server().await.unwrap();
    let clients = SilaClients::grpc(endpoint);
    assert!(!clients.is_simulator());

    // dispense
    let r = clients
        .execute(&act("dispense", json!({"vessel_id": "tube_1", "volume_ul": 100.0, "reagent": "NaCl"})))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert_eq!(r["actual_volume_dispensed"], 100.0);

    // read absorbance after filling
    let r = clients
        .execute(&act("read_absorbance", json!({"vessel_id": "tube_1", "wavelength_nm": 500.0})))
        .await
        .unwrap();
    assert_eq!(r["success"], true);
    assert!(r["absorbance_value"].as_f64().unwrap() > 0.0);

    // set + read temperature
    clients
        .execute(&act("set_temperature", json!({"device_id": "plate1", "target_temp_c": 37.0})))
        .await
        .unwrap();
    let r = clients
        .execute(&act("read_temperature", json!({"device_id": "plate1"})))
        .await
        .unwrap();
    assert_eq!(r["current_temp_c"], 37.0);

    server.abort();
}

#[tokio::test]
async fn overflow_is_reported_in_band_over_grpc() {
    let (endpoint, server) = spawn_mock_server().await.unwrap();
    let clients = SilaClients::grpc(endpoint);
    // tube_1 capacity is 2000 µL in the sim; 9999 overflows.
    let r = clients
        .execute(&act("dispense", json!({"vessel_id": "tube_1", "volume_ul": 9999.0})))
        .await
        .unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error_message"].as_str().unwrap().contains("overflow"));
    server.abort();
}
