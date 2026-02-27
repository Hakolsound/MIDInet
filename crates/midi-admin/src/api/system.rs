use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use tracing::{error, info};

use crate::state::AppState;

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
pub async fn run_update(State(state): State<AppState>) -> Json<Value> {
    // Safety checks
    let clients = state.inner.clients.read().await;
    let client_count = clients.len();
    drop(clients);

    let midi = state.inner.midi_metrics.read().await;
    let midi_rate = midi.messages_in_per_sec;
    drop(midi);

    // Use the installed midinet-update command (installed by pi-update.sh / pi-provision.sh).
    // Falls back to locating the script in the source tree if the command isn't installed yet.
    let script_path = if std::path::Path::new("/usr/local/bin/midinet-update").exists() {
        std::path::PathBuf::from("/usr/local/bin/midinet-update")
    } else if let Some(src_dir) = find_src_dir() {
        let script = src_dir.join("scripts").join("pi-update.sh");
        if script.exists() {
            script
        } else {
            return Json(json!({
                "success": false,
                "error": "Update script not found. Run: sudo install -m 755 <repo>/scripts/pi-update.sh /usr/local/bin/midinet-update",
            }));
        }
    } else {
        return Json(json!({
            "success": false,
            "error": "Update script not found. Run: sudo install -m 755 <repo>/scripts/pi-update.sh /usr/local/bin/midinet-update",
        }));
    };

    info!(script = %script_path.display(), "Launching host update");

    // Spawn the update script as a detached background process
    match std::process::Command::new("sudo")
        .args(["bash", &script_path.to_string_lossy(), "--force"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {
            info!(
                clients = client_count,
                midi_rate = midi_rate,
                "Update script launched — services will restart"
            );
            Json(json!({
                "success": true,
                "message": "Update started. Services will restart momentarily.",
                "clients": client_count,
                "midi_rate": midi_rate,
            }))
        }
        Err(e) => {
            error!(error = %e, "Failed to launch update script");
            Json(json!({
                "success": false,
                "error": format!("Failed to launch update: {}", e),
            }))
        }
    }
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

    // Query remote using ls-remote with the URL directly (no repo access needed)
    let latest = match std::process::Command::new("git")
        .args(["ls-remote", "--heads", &remote_url, &format!("refs/heads/{}", branch)])
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
            // Output format: "<full-hash>\trefs/heads/<branch>"
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .chars()
                .take(7)
                .collect::<String>()
        }
    };

    if current.is_empty() || latest.is_empty() {
        return json!({
            "available": false,
            "error": "Failed to read git hashes",
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

