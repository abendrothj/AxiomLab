fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile SiLA 2 proto definitions for all 6 instruments
    // We need to compile the framework proto first, then each instrument.
    // The key challenge: SiLA 2 protos use deeply nested package paths like
    // sila2.org.axiomlab.liquidhandling.liquidhandler.v1 which creates
    // super::super::super references. We compile everything together so
    // prost creates a single coherent module tree.

    let all_protos = vec![
        "proto/SiLAFramework.proto",
        "proto/SiLABinaryTransfer.proto",
        "proto/LiquidHandler.proto",
        "proto/RoboticArm.proto",
        "proto/Spectrophotometer.proto",
        "proto/Incubator.proto",
        "proto/Centrifuge.proto",
        "proto/PHMeter.proto",
    ];

    tonic_build::configure()
        .build_server(false) // We only need clients
        .compile_protos(&all_protos, &["proto/"])?;

    Ok(())
}
