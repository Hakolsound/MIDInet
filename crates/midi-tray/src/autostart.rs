/// Windows auto-start management via the Registry Run key.
///
/// Reads/writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\MIDInet`
/// to control whether midi-tray.exe launches on user login.

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(target_os = "windows")]
mod windows_impl {
    use tracing::{info, warn};
    use winreg::enums::*;
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "MIDInet";

    /// Check if auto-start is currently enabled.
    pub fn is_enabled() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run_key) = hkcu.open_subkey(RUN_KEY) {
            run_key.get_value::<String, _>(VALUE_NAME).is_ok()
        } else {
            false
        }
    }

    /// Enable auto-start (writes current exe path to registry).
    pub fn enable() -> Result<(), String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Cannot get exe path: {}", e))?;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(|e| format!("Cannot open Run key: {}", e))?;

        // Quote the path in case it contains spaces
        let value = format!("\"{}\"", exe_path.display());
        run_key
            .set_value(VALUE_NAME, &value)
            .map_err(|e| format!("Cannot set registry value: {}", e))?;

        info!(path = %exe_path.display(), "Auto-start enabled");
        Ok(())
    }

    /// Disable auto-start (removes the registry value).
    pub fn disable() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run_key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_WRITE) {
            match run_key.delete_value(VALUE_NAME) {
                Ok(()) => info!("Auto-start disabled"),
                Err(e) => warn!("Registry value not found (already disabled?): {}", e),
            }
        }
        Ok(())
    }

    /// Toggle auto-start. Returns the new state.
    pub fn toggle() -> Result<bool, String> {
        if is_enabled() {
            disable()?;
            Ok(false)
        } else {
            enable()?;
            Ok(true)
        }
    }
}

// No-op on non-Windows platforms
#[cfg(not(target_os = "windows"))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn toggle() -> Result<bool, String> {
    Ok(false)
}
