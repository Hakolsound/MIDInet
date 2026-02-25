fn main() {
    // Tell the linker where to find teVirtualMIDI.lib on Windows.
    // Searches: SDK install path, then project root as fallback.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let search_paths = [
            r"C:\Program Files (x86)\Tobias Erichsen\teVirtualMIDISDK\C-Binding\x64",
            r"C:\Program Files\Tobias Erichsen\teVirtualMIDISDK\C-Binding\x64",
        ];

        for path in &search_paths {
            if std::path::Path::new(path).join("teVirtualMIDI.lib").exists() {
                println!("cargo:rustc-link-search=native={}", path);
                return;
            }
        }

        // Fallback: check if .lib was copied next to Cargo.toml or in the workspace root
        if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
            println!("cargo:rustc-link-search=native={}", manifest_dir);
        }
        if let Ok(workspace_dir) = std::env::var("CARGO_WORKSPACE_DIR") {
            println!("cargo:rustc-link-search=native={}", workspace_dir);
        }

        eprintln!(
            "warning: teVirtualMIDI.lib not found in SDK paths. \
             Install the teVirtualMIDI SDK from https://www.tobias-erichsen.de/software/virtualmidi.html"
        );
    }
}
