fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate gRPC clients for the instrument services. Thin wrappers in
    // `grpc.rs` adapt these to the unified `SilaClients::execute` interface.
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/instruments.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/instruments.proto");

    // Generate clients for the full SiLA 2 protocol used by `sila_sim`.
    // These protos intentionally live in this crate so the Rust build does not
    // depend on a Python virtualenv or installed sila2 package.
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(
            &[
                "proto/sila2/SiLAFramework.proto",
                "proto/sila2/LiquidHandler.proto",
                "proto/sila2/Spectrophotometer.proto",
                "proto/sila2/Incubator.proto",
            ],
            &["proto/sila2"],
        )?;
    println!("cargo:rerun-if-changed=proto/sila2/SiLAFramework.proto");
    println!("cargo:rerun-if-changed=proto/sila2/LiquidHandler.proto");
    println!("cargo:rerun-if-changed=proto/sila2/Spectrophotometer.proto");
    println!("cargo:rerun-if-changed=proto/sila2/Incubator.proto");
    Ok(())
}
