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

    // Return safety info so the frontend can display warnings,
    // but don't block — the frontend handles confirmation
    let src_dir = find_src_dir();
    let script = src_dir
        .as_ref()
        .map(|d| d.join("scripts").join("pi-update.sh"));

    if script.as_ref().map_or(true, |s| !s.exists()) {
        return Json(json!({
            "success": false,
            "error": "Update script not found",
        }));
    }

    let script_path = script.unwrap();
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
    let src_dir = match find_src_dir() {
        Some(d) => d,
        None => {
            return json!({
                "available": false,
                "error": "Source directory not found",
            })
        }
    };

    let branch = midi_protocol::GIT_BRANCH;

    // Query remote using ls-remote (read-only — no write access to .git needed).
    // This avoids permission issues when the admin service runs as a different user.
    let latest = match std::process::Command::new("git")
        .args(["ls-remote", "--heads", "origin", &format!("refs/heads/{}", branch)])
        .current_dir(&src_dir)
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

    let current = git_rev_parse(&src_dir, "HEAD");

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

    // Try to collect changelog from locally-cached refs (may be stale, that's OK)
    let changelog: Vec<String> = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("HEAD..origin/{}", branch)])
        .current_dir(&src_dir)
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .map(|s| s.lines().take(10).map(|l| l.to_string()).collect())
        .unwrap_or_default();

    json!({
        "available": true,
        "current_hash": current,
        "latest_hash": latest,
        "changelog": changelog,
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

fn git_rev_parse(dir: &std::path::Path, rev: &str) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", rev])
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
