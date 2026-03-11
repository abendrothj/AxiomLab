//! Build script for verus_proofs.
//!
//! Parses `verus_verified/lab_safety.rs` (the Verus-verified source of
//! truth) and extracts `pub const` declarations into a generated Rust
//! file.  This guarantees the runtime crate uses EXACTLY the same
//! constants that Verus formally verified.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let verus_source = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .join("verus_verified")
        .join("lab_safety.rs");

    println!("cargo::rerun-if-changed={}", verus_source.display());

    let source = fs::read_to_string(&verus_source).unwrap_or_else(|e| {
        panic!(
            "Cannot read Verus source at {}: {e}\n\
             The verus_verified/lab_safety.rs file is the single source of truth \
             for all hardware safety constants.",
            verus_source.display()
        )
    });

    let out_dir = env::var("OUT_DIR").unwrap();
    let generated_path = Path::new(&out_dir).join("verus_constants.rs");

    let mut generated = String::new();
    generated.push_str(
        "// AUTO-GENERATED from verus_verified/lab_safety.rs — DO NOT EDIT\n\
         //\n\
         // These constants are extracted from the Verus-verified source file.\n\
         // Any change to hardware safety bounds MUST be made in\n\
         // verus_verified/lab_safety.rs and re-verified with the Verus compiler.\n\n",
    );

    // Extract all `pub const NAME: TYPE = VALUE;` lines from inside verus! { }
    let mut in_verus_block = false;
    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("verus!") {
            in_verus_block = true;
            continue;
        }
        if trimmed == "} // verus!" {
            break;
        }

        if in_verus_block && trimmed.starts_with("pub const ") {
            generated.push_str(trimmed);
            generated.push('\n');
        }
    }

    // Also record the source path for the consistency test
    generated.push_str(&format!(
        "\n/// Path to the Verus source file these constants were extracted from.\n\
         pub const VERUS_SOURCE_PATH: &str = {:?};\n",
        verus_source.to_str().unwrap()
    ));

    fs::write(&generated_path, &generated).unwrap_or_else(|e| {
        panic!("Cannot write generated constants to {}: {e}", generated_path.display())
    });
}
