fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate gRPC clients for the instrument services. Thin wrappers in
    // `grpc.rs` adapt these to the unified `SilaClients::execute` interface.
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/instruments.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/instruments.proto");
    Ok(())
}
