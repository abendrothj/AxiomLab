fn main() {
    // Gate the `unsafe-bypass` feature out of release builds.
    // This feature disables Ed25519 manifest signature verification and must
    // never reach production.  The build will hard-fail if someone accidentally
    // enables it in a --release build.
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let bypass_enabled = std::env::var("CARGO_FEATURE_UNSAFE_BYPASS").is_ok();

    if profile == "release" && bypass_enabled {
        panic!(
            "\n\
            ╔══════════════════════════════════════════════════════════════╗\n\
            ║  FATAL: `unsafe-bypass` feature enabled in a release build.  ║\n\
            ║                                                              ║\n\
            ║  This feature disables Ed25519 manifest signature            ║\n\
            ║  verification and MUST NOT be enabled outside of tests.      ║\n\
            ║                                                              ║\n\
            ║  Remove `--features unsafe-bypass` from your build command.  ║\n\
            ╚══════════════════════════════════════════════════════════════╝\n"
        );
    }
}
