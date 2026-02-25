fn main() {
    // Tell the linker where to find teVirtualMIDI.lib on Windows.
    // Searches: bundled lib in repo first, then SDK install paths as fallback.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

        let search_paths = [
            // Bundled in repo (preferred — no SDK install needed)
            format!(r"{}\lib\x64", manifest_dir),
            // SDK install paths (fallback)
            r"C:\Program Files (x86)\Tobias Erichsen\teVirtualMIDISDK\C-Binding\x64".to_string(),
            r"C:\Program Files\Tobias Erichsen\teVirtualMIDISDK\C-Binding\x64".to_string(),
        ];

        for path in &search_paths {
            if std::path::Path::new(path).join("teVirtualMIDI.lib").exists() {
                println!("cargo:rustc-link-search=native={}", path);
                return;
            }
        }

        eprintln!(
            "warning: teVirtualMIDI.lib not found. \
             Either place it in crates/midi-client/lib/x64/ or install the SDK from \
             https://www.tobias-erichsen.de/software/virtualmidi.html"
        );
    }
}
