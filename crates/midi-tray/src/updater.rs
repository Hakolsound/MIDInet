/// Self-update logic for the system tray.
///
/// Checks for available updates by comparing the local git HEAD to the remote,
/// and launches the platform-specific install script to perform the actual update.
///
/// Windows: launches PowerShell install script.
/// macOS: launches bash install script in Terminal.app.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tracing::{error, info, warn};

/// Result of an update check.
pub struct UpdateCheckResult {
    pub available: bool,
    pub current_hash: String,
    pub latest_hash: String,
    pub changelog: Vec<String>,
    /// If non-empty, the check failed and this contains a user-visible error message.
    pub error: String,
}

/// Find the MIDInet source directory.
/// Windows: `%LOCALAPPDATA%\MIDInet\src\` (cloned by the install script).
/// macOS: `~/.midinet/src/` (cloned by the install script).
fn find_src_dir() -> Option<PathBuf> {
    // macOS / Linux: ~/.midinet/src
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let src = PathBuf::from(home).join(".midinet").join("src");
            if src.join(".git").exists() {
                return Some(src);
            }
        }
    }

    // Windows: %LOCALAPPDATA%\MIDInet\src
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            let src = PathBuf::from(appdata).join("MIDInet").join("src");
            if src.join(".git").exists() {
                return Some(src);
            }
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

/// Get the git remote URL from the local repo.
fn get_remote_url(src_dir: &PathBuf) -> Option<String> {
    Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(src_dir)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn make_error(msg: &str) -> UpdateCheckResult {
    UpdateCheckResult {
        available: false,
        current_hash: String::new(),
        latest_hash: String::new(),
        changelog: Vec::new(),
        error: msg.to_string(),
    }
}

/// Check for available updates using ls-remote (no fetch, no credential prompt).
/// This is a blocking call — run in a background thread.
pub fn check_for_update() -> UpdateCheckResult {
    let src_dir = match find_src_dir() {
        Some(d) => d,
        None => {
            error!("Cannot find MIDInet source directory for update check");
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            return make_error(
                "Source directory not found.\n\n\
                 Expected: ~/.midinet/src/\n\
                 Run the installer to set it up.",
            );
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            return make_error(
                "Source directory not found.\n\n\
                 Expected: %LOCALAPPDATA%\\MIDInet\\src\\\n\
                 Run the installer to set it up.",
            );
        }
    };

    let branch = midi_protocol::GIT_BRANCH;

    // Get current HEAD hash from the local repo (read-only, fast)
    let current_hash = git_rev_parse(&src_dir, "HEAD");
    if current_hash.is_empty() {
        error!("Failed to get current HEAD hash");
        return make_error("Failed to read current git hash.");
    }

    // Get remote URL from the local repo
    let remote_url = match get_remote_url(&src_dir) {
        Some(url) => url,
        None => {
            error!("Failed to get git remote URL");
            return make_error("Failed to read git remote URL from repo.");
        }
    };

    // Query remote using ls-remote (read-only, no credential prompt for public repos).
    // Use a child process with a timeout to avoid hanging forever.
    info!(remote = %remote_url, branch = %branch, "Checking for updates via ls-remote");
    let ls_remote = Command::new("git")
        .args(["ls-remote", &remote_url, &format!("refs/heads/{}", branch)])
        .current_dir(&src_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let child = match ls_remote {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to spawn git ls-remote");
            return make_error(&format!("Failed to run git: {}", e));
        }
    };

    // Wait with a 15-second timeout
    let output = wait_with_timeout(child, Duration::from_secs(15));
    let output = match output {
        Some(Ok(o)) => o,
        Some(Err(e)) => {
            error!(error = %e, "git ls-remote failed");
            return make_error(&format!("git ls-remote failed: {}", e));
        }
        None => {
            warn!("git ls-remote timed out after 15 seconds");
            return make_error("Update check timed out.\nCheck your internet connection.");
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(exit = %output.status, stderr = %stderr.trim(), "git ls-remote failed");
        return make_error(&format!("git ls-remote failed: {}", stderr.trim()));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let latest_hash: String = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .take(7)
        .collect();

    if latest_hash.is_empty() {
        error!(raw = %raw.trim(), "git ls-remote returned no matching refs");
        return make_error(&format!(
            "No branch '{}' found on remote.\nls-remote output: {}",
            branch,
            raw.trim()
        ));
    }

    if current_hash == latest_hash {
        info!("Already up to date ({})", current_hash);
        return UpdateCheckResult {
            available: false,
            current_hash,
            latest_hash,
            changelog: Vec::new(),
            error: String::new(),
        };
    }

    // Try to get changelog from locally-cached refs (may be stale, that's OK)
    let changelog = Command::new("git")
        .args([
            "log",
            "--oneline",
            &format!("HEAD..origin/{}", branch),
        ])
        .current_dir(&src_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
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
        error: String::new(),
    }
}

/// Wait for a child process with a timeout. Returns None if timed out (and kills the process).
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::io::Result<std::process::Output>> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process exited — collect output
                return Some(child.wait_with_output());
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Some(Err(e)),
        }
    }
}

/// Launch the platform-specific install script and exit the current process.
/// The script will stop the tray, rebuild, and restart it.
pub fn run_update() -> bool {
    let src_dir = match find_src_dir() {
        Some(d) => d,
        None => {
            error!("Cannot find MIDInet source directory for update");
            return false;
        }
    };

    #[cfg(target_os = "windows")]
    let result = {
        let script = src_dir.join("scripts").join("client-install-windows.ps1");
        if !script.exists() {
            error!(path = %script.display(), "Install script not found");
            return false;
        }
        info!(script = %script.display(), "Launching update script");
        // Launch PowerShell in a new window so the user can see progress
        Command::new("powershell")
            .args([
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &script.to_string_lossy(),
            ])
            .spawn()
    };

    #[cfg(target_os = "macos")]
    let result = {
        let script = src_dir.join("scripts").join("client-install-macos.sh");
        if !script.exists() {
            error!(path = %script.display(), "Install script not found");
            return false;
        }
        info!(script = %script.display(), "Launching update script in Terminal.app");
        // Launch in Terminal.app so the user can see progress
        Command::new("open")
            .args(["-a", "Terminal", &script.to_string_lossy()])
            .spawn()
    };

    #[cfg(target_os = "linux")]
    let result = {
        let script = src_dir.join("scripts").join("client-install-linux.sh");
        if !script.exists() {
            error!(path = %script.display(), "Install script not found");
            return false;
        }
        info!(script = %script.display(), "Launching update script in terminal");
        find_terminal_and_run(&script)
    };

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let result: Result<std::process::Child, std::io::Error> = {
        error!("Update not supported on this platform");
        return false;
    };

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

/// Try to launch a script in a terminal emulator.
/// Tries $TERMINAL, x-terminal-emulator, gnome-terminal, xterm in order.
#[cfg(target_os = "linux")]
fn find_terminal_and_run(script: &std::path::Path) -> Result<std::process::Child, std::io::Error> {
    let script_str = script.to_string_lossy();

    // 1. $TERMINAL env var
    if let Ok(term) = std::env::var("TERMINAL") {
        if let Ok(child) = Command::new(&term).args(["-e", &format!("bash {}", script_str)]).spawn()
        {
            return Ok(child);
        }
    }

    // 2. x-terminal-emulator (Debian/Ubuntu alternatives system)
    if let Ok(child) = Command::new("x-terminal-emulator")
        .args(["-e", &format!("bash {}", script_str)])
        .spawn()
    {
        return Ok(child);
    }

    // 3. gnome-terminal
    if let Ok(child) = Command::new("gnome-terminal")
        .args(["--", "bash", &*script_str])
        .spawn()
    {
        return Ok(child);
    }

    // 4. xterm (widely available fallback)
    Command::new("xterm")
        .args(["-e", "bash", &*script_str])
        .spawn()
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
