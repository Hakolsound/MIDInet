/// Auto-start management.
///
/// Windows: Registry Run key (`HKCU\...\Run\MIDInet`).
/// macOS: LaunchAgent plist (`~/Library/LaunchAgents/co.hakol.midinet-client.plist`).

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

#[cfg(target_os = "macos")]
pub use macos_impl::*;

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::process::Command;
    use tracing::{info, warn};

    const CLIENT_LABEL: &str = "co.hakol.midinet-client";

    fn plist_path() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{}/Library/LaunchAgents/{}.plist", home, CLIENT_LABEL)
    }

    /// Check if the client LaunchAgent is loaded (i.e. auto-starts at login).
    pub fn is_enabled() -> bool {
        // If the plist exists and is loaded, `launchctl list` will find it
        let path = plist_path();
        if !std::path::Path::new(&path).exists() {
            return false;
        }
        Command::new("launchctl")
            .args(["list", CLIENT_LABEL])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Enable auto-start (load the LaunchAgent plist).
    pub fn enable() -> Result<(), String> {
        let path = plist_path();
        if !std::path::Path::new(&path).exists() {
            return Err(format!("LaunchAgent plist not found: {}", path));
        }
        let uid = unsafe { libc::getuid() };
        let output = Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{}", uid), &path])
            .output()
            .map_err(|e| format!("Failed to run launchctl: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Error 37 = "already loaded" — treat as success
            if !stderr.contains("37") {
                warn!(stderr = %stderr.trim(), "launchctl bootstrap failed");
                return Err(format!("launchctl bootstrap failed: {}", stderr.trim()));
            }
        }
        info!("Auto-start enabled (LaunchAgent loaded)");
        Ok(())
    }

    /// Disable auto-start (unload the LaunchAgent plist).
    pub fn disable() -> Result<(), String> {
        let uid = unsafe { libc::getuid() };
        let target = format!("gui/{}/{}", uid, CLIENT_LABEL);
        let output = Command::new("launchctl")
            .args(["bootout", &target])
            .output()
            .map_err(|e| format!("Failed to run launchctl: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Error 3 = "not loaded" — treat as success
            if !stderr.contains("3:") {
                warn!(stderr = %stderr.trim(), "launchctl bootout failed");
            }
        }
        info!("Auto-start disabled (LaunchAgent unloaded)");
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

#[cfg(target_os = "linux")]
pub use linux_impl::*;

#[cfg(target_os = "linux")]
mod linux_impl {
    use tracing::{info, warn};

    fn desktop_entry_path() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{}/.config/autostart/midinet-tray.desktop", home)
    }

    fn midinet_bin_path() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{}/.midinet/bin/midinet-tray", home)
    }

    /// Check if auto-start is currently enabled (desktop entry exists).
    pub fn is_enabled() -> bool {
        std::path::Path::new(&desktop_entry_path()).exists()
    }

    /// Enable auto-start (create desktop entry).
    pub fn enable() -> Result<(), String> {
        let path = desktop_entry_path();
        let dir = std::path::Path::new(&path)
            .parent()
            .expect("desktop entry path has parent");
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create autostart dir: {}", e))?;
        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=MIDInet Tray\n\
             Comment=MIDInet system tray health monitor\n\
             Exec={}\n\
             Terminal=false\n\
             StartupNotify=false\n\
             X-GNOME-Autostart-enabled=true\n",
            midinet_bin_path()
        );
        std::fs::write(&path, content)
            .map_err(|e| format!("Cannot write desktop entry: {}", e))?;
        info!("Auto-start enabled (desktop entry created)");
        Ok(())
    }

    /// Disable auto-start (remove desktop entry).
    pub fn disable() -> Result<(), String> {
        let path = desktop_entry_path();
        match std::fs::remove_file(&path) {
            Ok(()) => info!("Auto-start disabled (desktop entry removed)"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!("Desktop entry not found (already disabled?)");
            }
            Err(e) => return Err(format!("Cannot remove desktop entry: {}", e)),
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

// No-op on unsupported platforms
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[allow(dead_code)]
pub fn toggle() -> Result<bool, String> {
    Ok(false)
}
