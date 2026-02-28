use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::state::AppState;

pub(crate) const UPDATE_LOG_PATH: &str = "/var/lib/midinet/update.log";

/// GET /api/system/update-check — check if a newer version is available on origin.
pub async fn check_update() -> Json<Value> {
    let result = tokio::task::spawn_blocking(|| git_update_check())
        .await
        .unwrap_or_else(|e| {
            error!(error = %e, "Update check task panicked");
            json!({ "available": false, "error": "internal error" })
        });

    Json(result)
}

/// POST /api/system/update — pull latest code, rebuild, and restart services.
///
/// Instead of spawning `sudo` directly (which is blocked by NoNewPrivileges=true
/// in the systemd unit), we write a trigger file that `midinet-update.path`
/// watches. Systemd then starts `midinet-update.service` as root.
pub async fn run_update(State(state): State<AppState>) -> Json<Value> {
    // Safety checks
    let clients = state.inner.clients.read().await;
    let client_count = clients.len();
    drop(clients);

    let midi = state.inner.midi_metrics.read().await;
    let midi_rate = midi.messages_in_per_sec;
    drop(midi);

    // The update script must be installed at the well-known path.
    if !std::path::Path::new("/usr/local/bin/midinet-update").exists() {
        return Json(json!({
            "success": false,
            "error": "Update script not installed. Run: sudo install -m 755 <repo>/scripts/pi-update.sh /usr/local/bin/midinet-update",
        }));
    }

    // Check if an update is already running.
    let already_running = std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "midinet-update.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if already_running {
        return Json(json!({
            "success": false,
            "error": "An update is already in progress.",
        }));
    }

    // Verify the systemd path unit is active — required for web-triggered updates.
    // Installed by pi-update.sh v3.1+. If missing, the user needs one manual update.
    let path_unit_active = std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "midinet-update.path"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !path_unit_active {
        return Json(json!({
            "success": false,
            "error": "Update system not ready. Run 'sudo midinet-update' once from the terminal to install the required systemd units.",
        }));
    }

    // Create/truncate the log file for output capture.
    if let Err(e) = std::fs::File::create(UPDATE_LOG_PATH) {
        error!(error = %e, "Cannot create update log file");
        return Json(json!({
            "success": false,
            "error": format!("Cannot create log file: {}", e),
        }));
    }

    info!("Triggering host update via systemd path unit");

    // Write the trigger file. The midinet-update.path systemd unit watches
    // this file and starts midinet-update.service (which runs as root).
    // This avoids needing sudo from within the NoNewPrivileges=true sandbox.
    let trigger = format!("{:?}\n", std::time::SystemTime::now());
    match std::fs::write("/var/lib/midinet/update-trigger", trigger) {
        Ok(()) => {
            info!(
                clients = client_count,
                midi_rate = midi_rate,
                "Update triggered — streaming progress"
            );

            // Spawn background task to tail the log file and broadcast lines.
            tokio::spawn({
                let tx = state.inner.update_log_tx.clone();
                async move { tail_update_log(tx).await }
            });

            Json(json!({
                "success": true,
                "message": "Update started. Progress will be streamed.",
                "clients": client_count,
                "midi_rate": midi_rate,
            }))
        }
        Err(e) => {
            error!(error = %e, "Failed to write update trigger file");
            Json(json!({
                "success": false,
                "error": format!("Failed to trigger update: {}", e),
            }))
        }
    }
}

/// GET /api/system/update-status — check update progress and return the log.
/// Used by the frontend to backfill after reconnecting (admin restart mid-update).
pub async fn update_status() -> Json<Value> {
    let log_content = std::fs::read_to_string(UPDATE_LOG_PATH).unwrap_or_default();

    let lines: Vec<String> = log_content
        .lines()
        .map(|l| strip_ansi(l))
        .filter(|l| !l.is_empty())
        .collect();

    let step = parse_update_step(&log_content);
    let complete = log_content.contains("MIDInet updated and running");
    let failed = !complete
        && !log_content.is_empty()
        && (log_content.contains("exit 1")
            || log_content.contains("Build failed")
            || log_content.contains("error[E")
            || is_stale_log());

    let version = std::fs::read_to_string("/usr/local/bin/.midinet-version")
        .ok()
        .map(|s| s.trim().to_string());

    Json(json!({
        "complete": complete,
        "failed": failed,
        "step": step,
        "total_steps": 4,
        "lines": lines,
        "version": version,
    }))
}

/// Check if the update log file is stale (no writes in 5 minutes).
/// If so, the update script likely exited without the success marker.
fn is_stale_log() -> bool {
    std::fs::metadata(UPDATE_LOG_PATH)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| t.elapsed().unwrap_or_default().as_secs() > 300)
        .unwrap_or(false)
}

/// Parse the highest [N/4] step marker from update log output.
fn parse_update_step(log: &str) -> u8 {
    let mut step = 0u8;
    for line in log.lines() {
        // Match patterns like [1/4], [2/4], etc.
        if let Some(bracket) = line.find('[') {
            let rest = &line[bracket + 1..];
            if let Some(slash) = rest.find('/') {
                if let Ok(n) = rest[..slash].parse::<u8>() {
                    if n > step && n <= 4 {
                        step = n;
                    }
                }
            }
        }
    }
    step
}

/// Background task that tails the update log file and broadcasts new lines.
async fn tail_update_log(tx: tokio::sync::broadcast::Sender<String>) {
    use tokio::time::{sleep, Duration};

    let mut pos: usize = 0;

    loop {
        sleep(Duration::from_millis(200)).await;

        let content = match tokio::fs::read_to_string(UPDATE_LOG_PATH).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        if content.len() <= pos {
            // No new data — check if script finished
            if pos > 0 && is_stale_log() {
                debug!("Update log stale, stopping tail");
                break;
            }
            continue;
        }

        let new_text = &content[pos..];
        for line in new_text.lines() {
            let stripped = strip_ansi(line);
            if !stripped.is_empty() {
                let _ = tx.send(stripped);
            }
        }
        pos = content.len();

        // Check for completion
        if content.contains("MIDInet updated and running") {
            let _ = tx.send("__UPDATE_COMPLETE__".to_string());
            info!("Update completed successfully");
            break;
        }
        if content.contains("exit 1")
            || content.contains("Build failed")
            || content.contains("error[E")
        {
            let _ = tx.send("__UPDATE_FAILED__".to_string());
            warn!("Update script failed");
            break;
        }
    }
}

/// Strip ANSI escape sequences from a string.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI sequence — skip until terminator letter
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch.is_ascii_alphabetic() || ch == '~' || ch == '@' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC sequence — skip until BEL
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\x07' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn git_update_check() -> Value {
    let branch = midi_protocol::GIT_BRANCH;

    // Current hash: version stamp written by pi-update.sh, or compiled-in fallback
    let current = std::fs::read_to_string("/usr/local/bin/.midinet-version")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| midi_protocol::GIT_HASH.to_string());

    // Remote URL: marker written by pi-update.sh (no repo access needed)
    let remote_url = match std::fs::read_to_string("/var/lib/midinet/git-remote") {
        Ok(url) if !url.trim().is_empty() => url.trim().to_string(),
        _ => {
            // Try reading from repo if accessible
            find_src_dir()
                .and_then(|d| {
                    std::process::Command::new("git")
                        .args(["remote", "get-url", "origin"])
                        .current_dir(&d)
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_default()
        }
    };

    if remote_url.is_empty() {
        return json!({
            "available": false,
            "error": "Git remote URL not found. Run sudo midinet-update once to set it up.",
        });
    }

    // Query remote using ls-remote with the URL directly (no repo access needed).
    // Don't use --heads flag with explicit refs/heads/ pattern — they double-filter.
    let latest = match std::process::Command::new("git")
        .args(["ls-remote", &remote_url, &format!("refs/heads/{}", branch)])
        .output()
    {
        Err(e) => {
            return json!({
                "available": false,
                "error": format!("git ls-remote failed: {}", e),
            });
        }
        Ok(output) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return json!({
                "available": false,
                "error": format!("git ls-remote failed (exit {}): {}", output.status, stderr.trim()),
            });
        }
        Ok(output) => {
            let raw = String::from_utf8_lossy(&output.stdout);
            // Output format: "<full-hash>\trefs/heads/<branch>\n"
            let hash: String = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .chars()
                .take(7)
                .collect();
            if hash.is_empty() {
                error!(
                    remote_url = %remote_url,
                    branch = %branch,
                    raw_output = %raw.trim(),
                    "git ls-remote returned no matching refs"
                );
            }
            hash
        }
    };

    if current.is_empty() || latest.is_empty() {
        return json!({
            "available": false,
            "error": format!(
                "Failed to read git hashes (current={:?}, latest={:?}, url={:?}, branch={:?})",
                current, latest, remote_url, branch
            ),
        });
    }

    if current == latest {
        return json!({
            "available": false,
            "current_hash": current,
            "latest_hash": latest,
        });
    }

    json!({
        "available": true,
        "current_hash": current,
        "latest_hash": latest,
    })
}

fn find_src_dir() -> Option<std::path::PathBuf> {
    // Check marker file written by pi-update.sh / pi-provision.sh
    // This is the most reliable method since the update scripts know the exact path
    // and the marker is in a directory the admin service always has access to.
    // We trust the marker without checking .git — the admin user may not have
    // traversal permission to the parent dir until pi-update.sh fixes permissions.
    if let Ok(path) = std::fs::read_to_string("/var/lib/midinet/src-dir") {
        let dir = std::path::PathBuf::from(path.trim());
        if dir.as_os_str().len() > 1 {
            return Some(dir);
        }
    }

    let candidates = [
        std::path::PathBuf::from("/opt/midinet/src"),
        std::path::PathBuf::from("/home/pi/MIDInet"),
    ];

    for dir in &candidates {
        if dir.join(".git").exists() {
            return Some(dir.clone());
        }
    }

    // Fallback: walk up from the executable (works for dev and non-standard installs)
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..5 {
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

