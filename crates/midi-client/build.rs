fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        // Generate Rust bindings from the Windows MIDI Services winmd metadata.
        // The winmd file describes the Microsoft.Windows.Devices.Midi2 WinRT API.
        // Generated code depends on the `windows` and `windows-core` crates.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let winmd = format!("{}/winmd/Microsoft.Windows.Devices.Midi2.winmd", manifest_dir);
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let out_path = format!("{}/midi2_bindings.rs", out_dir);

        if std::path::Path::new(&winmd).exists() {
            println!("cargo:rerun-if-changed={}", winmd);

            match windows_bindgen::bindgen([
                "--in", &winmd,
                "--out", &out_path,
                "--filter", "Microsoft.Windows.Devices.Midi2",
            ]) {
                Ok(_) => println!("cargo:warning=Generated MIDI Services bindings from winmd"),
                Err(e) => {
                    // Non-fatal: if bindgen fails, the MIDI Services backend
                    // won't compile but teVirtualMIDI still works.
                    println!("cargo:warning=windows-bindgen failed (MIDI Services backend disabled): {}", e);

                    // Write an empty file so the include! doesn't fail
                    std::fs::write(&out_path, "// windows-bindgen failed — MIDI Services unavailable\n")
                        .expect("Failed to write placeholder bindings");
                }
            }
        } else {
            println!("cargo:warning=winmd file not found at {} — MIDI Services backend disabled", winmd);
            // Write empty placeholder
            std::fs::write(&out_path, "// winmd file not found — MIDI Services unavailable\n")
                .expect("Failed to write placeholder bindings");
        }
    }
}
