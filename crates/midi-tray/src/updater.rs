/// Self-update logic for the Windows tray.
///
/// Checks for available updates by comparing the local git HEAD to the remote,
/// and launches the PowerShell install script to perform the actual update.
///
/// Only compiled on Windows — macOS/Linux clients use their respective install scripts directly.

use std::path::PathBuf;
use std::process::Command;

use tracing::{error, info};

/// Result of an update check.
pub struct UpdateCheckResult {
    pub available: bool,
    pub current_hash: String,
    pub latest_hash: String,
    pub changelog: Vec<String>,
}

/// Find the MIDInet source directory.
/// Layout: `%LOCALAPPDATA%\MIDInet\src\` (cloned by the install script).
fn find_src_dir() -> Option<PathBuf> {
    // Primary: %LOCALAPPDATA%\MIDInet\src
    if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
        let src = PathBuf::from(appdata).join("MIDInet").join("src");
        if src.join(".git").exists() {
            return Some(src);
        }
    }

    // Fallback: walk up from the exe directory
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..4 {
            if let Some(ref d) = dir {
                if d.join(".git").exists() {
                    return dir;
                }
                dir = d.parent().map(|p| p.to_path_buf());
            }
        }
    }

    None
}

/// Check for available updates by fetching from origin and comparing HEAD.
/// This is a blocking call — run in a background thread.
pub fn check_for_update() -> UpdateCheckResult {
    let no_update = UpdateCheckResult {
        available: false,
        current_hash: String::new(),
        latest_hash: String::new(),
        changelog: Vec::new(),
    };

    let src_dir = match find_src_dir() {
        Some(d) => d,
        None => {
            error!("Cannot find MIDInet source directory for update check");
            return no_update;
        }
    };

    let branch = midi_protocol::GIT_BRANCH;

    // Fetch latest from origin
    info!(dir = %src_dir.display(), "Fetching updates from origin");
    let fetch = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(&src_dir)
        .output();

    if let Err(e) = fetch {
        error!(error = %e, "git fetch failed");
        return no_update;
    }

    // Get current HEAD hash
    let current_hash = git_rev_parse(&src_dir, "HEAD");
    let latest_hash = git_rev_parse(&src_dir, &format!("origin/{}", branch));

    if current_hash.is_empty() || latest_hash.is_empty() {
        error!("Failed to get git hashes");
        return no_update;
    }

    if current_hash == latest_hash {
        info!("Already up to date ({})", current_hash);
        return UpdateCheckResult {
            available: false,
            current_hash,
            latest_hash,
            changelog: Vec::new(),
        };
    }

    // Get changelog
    let changelog = Command::new("git")
        .args([
            "log",
            "--oneline",
            &format!("{}..{}", current_hash, latest_hash),
        ])
        .current_dir(&src_dir)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            s.lines()
                .take(10)
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    info!(
        current = %current_hash,
        latest = %latest_hash,
        changes = changelog.len(),
        "Update available"
    );

    UpdateCheckResult {
        available: true,
        current_hash,
        latest_hash,
        changelog,
    }
}

/// Launch the PowerShell install script and exit the current process.
/// The script will stop the tray, rebuild, and restart it.
pub fn run_update() -> bool {
    let src_dir = match find_src_dir() {
        Some(d) => d,
        None => {
            error!("Cannot find MIDInet source directory for update");
            return false;
        }
    };

    let script = src_dir.join("scripts").join("client-install-windows.ps1");
    if !script.exists() {
        error!(path = %script.display(), "Install script not found");
        return false;
    }

    info!(script = %script.display(), "Launching update script");

    // Launch PowerShell in a new window so the user can see progress
    let result = Command::new("powershell")
        .args([
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &script.to_string_lossy(),
        ])
        .spawn();

    match result {
        Ok(_) => {
            info!("Update script launched, tray will exit");
            true
        }
        Err(e) => {
            error!(error = %e, "Failed to launch update script");
            false
        }
    }
}

/// Format the update confirmation dialog text.
pub fn format_update_dialog(result: &UpdateCheckResult, admin_url: Option<&str>) -> String {
    let mut text = format!(
        "MIDInet Update Available\n\n\
         Current: {}\n\
         Latest:  {}\n",
        result.current_hash, result.latest_hash
    );

    if !result.changelog.is_empty() {
        text.push_str("\nChanges:\n");
        for line in &result.changelog {
            text.push_str(&format!("  {}\n", line));
        }
    }

    text.push_str(
        "\n\
         IMPORTANT: Both host (Raspberry Pi) and client (this PC)\n\
         must run the same version for proper functionality.\n\
         \n\
         After updating this client, update the host from the\n\
         Admin Dashboard > \"Update Host\" button.\n",
    );

    if let Some(url) = admin_url {
        text.push_str(&format!("Dashboard URL: {}\n", url));
    }

    text.push_str("\nProceed with client update?");

    text
}

fn git_rev_parse(dir: &PathBuf, rev: &str) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", rev])
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
