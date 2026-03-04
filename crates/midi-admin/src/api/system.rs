use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::state::AppState;

pub(crate) const UPDATE_LOG_PATH: &str = "/var/lib/midinet/update.log";
const RESTART_TRIGGER_PATH: &str = "/var/lib/midinet/restart-trigger";

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

// ── Mode change ──

#[derive(Deserialize)]
pub struct SetModeBody {
    pub mode: String,
}

/// POST /api/system/mode — change the host operational mode and restart.
///
/// Writes the new mode to host.toml, then triggers a host-only restart via
/// the midinet-restart.path systemd unit (same trigger pattern as updates).
/// Also schedules all connected clients for restart so they pick up the new mode.
pub async fn set_mode(
    State(state): State<AppState>,
    Json(body): Json<SetModeBody>,
) -> Json<Value> {
    // Validate mode
    let mode = body.mode.trim().to_lowercase();
    if !matches!(mode.as_str(), "single" | "redundant" | "multi") {
        return Json(json!({
            "success": false,
            "error": format!("Invalid mode '{}'. Must be 'single', 'redundant', or 'multi'.", mode),
        }));
    }

    // ── Eligibility check: block if any client has MIDI apps (Resolume) running ──
    {
        let clients = state.inner.clients.read().await;
        let blocking: Vec<_> = clients
            .iter()
            .filter(|c| c.midi_apps_active)
            .map(|c| json!({ "id": c.id, "hostname": c.hostname, "ip": c.ip }))
            .collect();
        if !blocking.is_empty() {
            let names: Vec<_> = clients
                .iter()
                .filter(|c| c.midi_apps_active)
                .map(|c| c.hostname.as_str())
                .collect();
            warn!(
                blocking = ?names,
                "Mode change blocked — MIDI apps active on clients"
            );
            return Json(json!({
                "success": false,
                "error": format!(
                    "Cannot change mode while MIDI applications are running on {} client(s). Close Resolume Arena first.",
                    blocking.len()
                ),
                "blocking_clients": blocking,
            }));
        }
    }

    // Read the config file, modify [host].mode, write back
    let config_path = state.inner.config_path.read().await.clone();

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(path = %config_path, error = %e, "Failed to read config file");
            return Json(json!({
                "success": false,
                "error": format!("Cannot read config: {}", e),
            }));
        }
    };

    let mut table: toml::Table = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to parse config TOML");
            return Json(json!({
                "success": false,
                "error": format!("Config parse error: {}", e),
            }));
        }
    };

    // Ensure [host] section exists and set mode
    let host_section = table
        .entry("host")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(ref mut host) = host_section {
        host.insert("mode".to_string(), toml::Value::String(mode.clone()));
    }

    let new_contents = match toml::to_string_pretty(&table) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "success": false,
                "error": format!("Failed to serialize config: {}", e),
            }));
        }
    };

    // Atomic write: temp file + rename
    let tmp_path = format!("{}.tmp", config_path);
    if let Err(e) = std::fs::write(&tmp_path, &new_contents) {
        error!(error = %e, "Failed to write temp config");
        return Json(json!({
            "success": false,
            "error": format!("Failed to write config: {}", e),
        }));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &config_path) {
        error!(error = %e, "Failed to rename temp config");
        let _ = std::fs::remove_file(&tmp_path);
        return Json(json!({
            "success": false,
            "error": format!("Failed to apply config: {}", e),
        }));
    }

    info!(mode = %mode, path = %config_path, "Operational mode changed in config");

    // Update the authoritative configured_mode — this is what get_status()
    // reads, independent of mDNS discovery which can return stale data.
    *state.inner.configured_mode.write().await = mode.clone();

    // Also update the in-memory host state for consistency.
    {
        let mut hosts = state.inner.hosts.write().await;
        for host in hosts.iter_mut() {
            host.operational_mode = mode.clone();
        }
    }

    // Restart the host so it picks up the new mode.
    // Primary: SIGTERM the host process directly — both services run as the midi
    // user, so signals are permitted. systemd Restart=always brings it back with
    // the updated config. Also write the trigger file for midinet-restart.path
    // as a belt-and-suspenders backup.

    let trigger = format!("mode={}\n{:?}\n", mode, std::time::SystemTime::now());
    if let Err(e) = std::fs::write(RESTART_TRIGGER_PATH, &trigger) {
        warn!(error = %e, "Failed to write restart trigger file (non-fatal)");
    }

    let sigterm_ok = signal_host_process();
    if !sigterm_ok {
        warn!("SIGTERM failed — relying on path unit trigger for restart");
    }

    // Schedule all connected clients for restart so they pick up the new mode
    let client_count = {
        let clients = state.inner.clients.read().await;
        let ids: Vec<u32> = clients.iter().map(|c| c.id).collect();
        let count = ids.len();
        let mut pending = state.inner.pending_restarts.write().await;
        pending.extend(ids);
        count
    };

    info!(
        mode = %mode,
        sigterm = sigterm_ok,
        clients_to_restart = client_count,
        "Host restart initiated for mode change"
    );
    Json(json!({
        "success": true,
        "restarting": true,
        "mode": mode,
        "clients_restarting": client_count,
    }))
}

/// Send SIGTERM to the host process by reading its PID from systemd.
/// Returns true if the signal was sent successfully.
fn signal_host_process() -> bool {
    // Ask systemd for the host's main PID
    let output = match std::process::Command::new("systemctl")
        .args(["show", "-p", "MainPID", "--value", "midinet-host.service"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "Failed to query host PID from systemd");
            return false;
        }
    };

    let pid_str = String::from_utf8_lossy(&output.stdout);
    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) if p > 0 => p,
        _ => {
            warn!(raw = %pid_str.trim(), "Host PID not found or zero");
            return false;
        }
    };

    // Send SIGTERM — both services run as the midi user so this is permitted.
    // systemd Restart=always will restart the host with the updated config.
    match std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
    {
        Ok(s) if s.success() => {
            info!(pid = pid, "Sent SIGTERM to midi-host process");
            true
        }
        Ok(s) => {
            warn!(pid = pid, exit = ?s.code(), "kill command failed");
            false
        }
        Err(e) => {
            warn!(pid = pid, error = %e, "Failed to execute kill command");
            false
        }
    }
}

// ── Host Redundancy Toggle ──

#[derive(Deserialize)]
pub struct SetHostRedundancyBody {
    pub enabled: bool,
}

/// GET /api/system/host-redundancy — return current host redundancy state.
pub async fn get_host_redundancy(
    State(state): State<AppState>,
) -> Json<Value> {
    let enabled = *state.inner.host_redundancy_enabled.read().await;
    Json(json!({ "host_redundancy": enabled }))
}

/// POST /api/system/host-redundancy — toggle host redundancy and restart.
pub async fn set_host_redundancy(
    State(state): State<AppState>,
    Json(body): Json<SetHostRedundancyBody>,
) -> Json<Value> {
    let config_path = state.inner.config_path.read().await.clone();

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(path = %config_path, error = %e, "Failed to read config file");
            return Json(json!({
                "success": false,
                "error": format!("Cannot read config: {}", e),
            }));
        }
    };

    let mut table: toml::Table = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to parse config TOML");
            return Json(json!({
                "success": false,
                "error": format!("Config parse error: {}", e),
            }));
        }
    };

    // Ensure [host] section exists and set host_redundancy
    let host_section = table
        .entry("host")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(ref mut host) = host_section {
        host.insert(
            "host_redundancy".to_string(),
            toml::Value::Boolean(body.enabled),
        );
    }

    let new_contents = match toml::to_string_pretty(&table) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "success": false,
                "error": format!("Failed to serialize config: {}", e),
            }));
        }
    };

    // Atomic write: temp file + rename
    let tmp_path = format!("{}.tmp", config_path);
    if let Err(e) = std::fs::write(&tmp_path, &new_contents) {
        error!(error = %e, "Failed to write temp config");
        return Json(json!({
            "success": false,
            "error": format!("Failed to write config: {}", e),
        }));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &config_path) {
        error!(error = %e, "Failed to rename temp config");
        let _ = std::fs::remove_file(&tmp_path);
        return Json(json!({
            "success": false,
            "error": format!("Failed to apply config: {}", e),
        }));
    }

    info!(
        host_redundancy = body.enabled,
        path = %config_path,
        "Host redundancy setting changed in config"
    );

    // Update in-memory state
    *state.inner.host_redundancy_enabled.write().await = body.enabled;

    // Restart host to pick up the new setting
    let trigger = format!(
        "host_redundancy={}\n{:?}\n",
        body.enabled,
        std::time::SystemTime::now()
    );
    if let Err(e) = std::fs::write(RESTART_TRIGGER_PATH, &trigger) {
        warn!(error = %e, "Failed to write restart trigger file (non-fatal)");
    }

    let sigterm_ok = signal_host_process();
    if !sigterm_ok {
        warn!("SIGTERM failed — relying on path unit trigger for restart");
    }

    info!(
        host_redundancy = body.enabled,
        sigterm = sigterm_ok,
        "Host restart initiated for host redundancy change"
    );
    Json(json!({
        "success": true,
        "restarting": true,
        "host_redundancy": body.enabled,
    }))
}

// ── Device Highway Management ──

#[derive(Deserialize)]
pub struct DeviceHighwayEntry {
    pub name: String,
    pub device: String,
}

#[derive(Deserialize)]
pub struct SetDeviceHighwaysBody {
    pub devices: Vec<DeviceHighwayEntry>,
}

/// PUT /api/settings/device-highways — set the [[midi.devices]] list in the config.
/// Writes to the TOML file and restarts the host so it picks up the changes.
pub async fn set_device_highways(
    State(state): State<AppState>,
    Json(body): Json<SetDeviceHighwaysBody>,
) -> Json<Value> {
    if body.devices.is_empty() {
        return Json(json!({
            "success": false,
            "error": "At least one device highway is required.",
        }));
    }
    if body.devices.len() > 16 {
        return Json(json!({
            "success": false,
            "error": "Maximum 16 device highways supported.",
        }));
    }

    let config_path = state.inner.config_path.read().await.clone();

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(path = %config_path, error = %e, "Failed to read config file");
            return Json(json!({
                "success": false,
                "error": format!("Cannot read config: {}", e),
            }));
        }
    };

    let mut table: toml::Table = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to parse config TOML");
            return Json(json!({
                "success": false,
                "error": format!("Config parse error: {}", e),
            }));
        }
    };

    // Build [[midi.devices]] array
    let devices_array: Vec<toml::Value> = body
        .devices
        .iter()
        .map(|d| {
            let mut t = toml::Table::new();
            t.insert("name".to_string(), toml::Value::String(d.name.clone()));
            t.insert("device".to_string(), toml::Value::String(d.device.clone()));
            toml::Value::Table(t)
        })
        .collect();

    // Ensure [midi] section exists
    let midi_section = table
        .entry("midi")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(ref mut midi) = midi_section {
        midi.insert(
            "devices".to_string(),
            toml::Value::Array(devices_array),
        );
    }

    let new_contents = match toml::to_string_pretty(&table) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "success": false,
                "error": format!("Failed to serialize config: {}", e),
            }));
        }
    };

    // Atomic write
    let tmp_path = format!("{}.tmp", config_path);
    if let Err(e) = std::fs::write(&tmp_path, &new_contents) {
        return Json(json!({
            "success": false,
            "error": format!("Failed to write config: {}", e),
        }));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &config_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Json(json!({
            "success": false,
            "error": format!("Failed to apply config: {}", e),
        }));
    }

    // Update in-memory configured_devices
    let device_names: Vec<String> = body.devices.iter().map(|d| d.name.clone()).collect();
    info!(devices = ?device_names, path = %config_path, "Device highways updated in config");
    *state.inner.configured_devices.write().await = device_names;

    // Restart host to pick up new device config
    let trigger = format!("highways\n{:?}\n", std::time::SystemTime::now());
    if let Err(e) = std::fs::write(RESTART_TRIGGER_PATH, &trigger) {
        warn!(error = %e, "Failed to write restart trigger file (non-fatal)");
    }
    let sigterm_ok = signal_host_process();

    info!(sigterm = sigterm_ok, "Host restart initiated for highway change");
    Json(json!({
        "success": true,
        "restarting": true,
        "device_count": body.devices.len(),
    }))
}

/// GET /api/settings/device-highways — return current [[midi.devices]] from config.
pub async fn get_device_highways(
    State(state): State<AppState>,
) -> Json<Value> {
    let config_path = state.inner.config_path.read().await.clone();

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return Json(json!({ "devices": [] })),
    };

    let table: toml::Table = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(_) => return Json(json!({ "devices": [] })),
    };

    let mut devices = Vec::new();
    if let Some(arr) = table
        .get("midi")
        .and_then(|m| m.as_table())
        .and_then(|m| m.get("devices"))
        .and_then(|d| d.as_array())
    {
        for entry in arr {
            if let Some(t) = entry.as_table() {
                devices.push(json!({
                    "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "device": t.get("device").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            }
        }
    }

    Json(json!({ "devices": devices }))
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

// ── Protected Apps Catalog ──

#[allow(dead_code)]
struct CatalogApp {
    id: &'static str,
    name: &'static str,
    category: &'static str,
    win_process: &'static str,
    mac_process: &'static str,
    linux_process: &'static str,
    win_install_hint: &'static str,
    mac_install_hint: &'static str,
}

const PROTECTED_APPS_CATALOG: &[CatalogApp] = &[
    // VJ / Media Server
    CatalogApp { id: "resolume-arena", name: "Resolume Arena", category: "VJ / Media Server", win_process: "Arena.exe", mac_process: "Arena", linux_process: "", win_install_hint: "Resolume Arena", mac_install_hint: "Resolume Arena" },
    CatalogApp { id: "resolume-avenue", name: "Resolume Avenue", category: "VJ / Media Server", win_process: "Avenue.exe", mac_process: "Avenue", linux_process: "", win_install_hint: "Resolume Avenue", mac_install_hint: "Resolume Avenue" },
    CatalogApp { id: "touchdesigner", name: "TouchDesigner", category: "VJ / Media Server", win_process: "TouchDesigner.exe", mac_process: "TouchDesigner", linux_process: "", win_install_hint: "TouchDesigner", mac_install_hint: "TouchDesigner" },
    CatalogApp { id: "madmapper", name: "MadMapper", category: "VJ / Media Server", win_process: "MadMapper.exe", mac_process: "MadMapper", linux_process: "", win_install_hint: "MadMapper", mac_install_hint: "MadMapper" },
    CatalogApp { id: "vdmx", name: "VDMX", category: "VJ / Media Server", win_process: "", mac_process: "VDMX5", linux_process: "", win_install_hint: "", mac_install_hint: "VDMX5" },
    CatalogApp { id: "millumin", name: "Millumin", category: "VJ / Media Server", win_process: "", mac_process: "Millumin", linux_process: "", win_install_hint: "", mac_install_hint: "Millumin" },
    CatalogApp { id: "disguise", name: "Disguise (d3)", category: "VJ / Media Server", win_process: "d3designer.exe", mac_process: "", linux_process: "", win_install_hint: "d3 Designer", mac_install_hint: "" },
    CatalogApp { id: "notch", name: "Notch", category: "VJ / Media Server", win_process: "Notch.exe", mac_process: "", linux_process: "", win_install_hint: "Notch", mac_install_hint: "" },
    // DAW
    CatalogApp { id: "ableton-live", name: "Ableton Live", category: "DAW", win_process: "Ableton Live", mac_process: "Live", linux_process: "", win_install_hint: "Ableton", mac_install_hint: "Ableton Live" },
    CatalogApp { id: "logic-pro", name: "Logic Pro", category: "DAW", win_process: "", mac_process: "Logic Pro", linux_process: "", win_install_hint: "", mac_install_hint: "Logic Pro" },
    CatalogApp { id: "cubase", name: "Cubase", category: "DAW", win_process: "Cubase", mac_process: "Cubase", linux_process: "", win_install_hint: "Cubase", mac_install_hint: "Cubase" },
    CatalogApp { id: "fl-studio", name: "FL Studio", category: "DAW", win_process: "FL64.exe", mac_process: "FL Studio", linux_process: "", win_install_hint: "FL Studio", mac_install_hint: "FL Studio" },
    CatalogApp { id: "reaper", name: "Reaper", category: "DAW", win_process: "reaper.exe", mac_process: "REAPER", linux_process: "reaper", win_install_hint: "REAPER", mac_install_hint: "REAPER" },
    CatalogApp { id: "bitwig", name: "Bitwig Studio", category: "DAW", win_process: "BitwigStudio.exe", mac_process: "Bitwig Studio", linux_process: "bitwig-studio", win_install_hint: "Bitwig Studio", mac_install_hint: "Bitwig Studio" },
    CatalogApp { id: "pro-tools", name: "Pro Tools", category: "DAW", win_process: "ProTools.exe", mac_process: "Pro Tools", linux_process: "", win_install_hint: "Pro Tools", mac_install_hint: "Pro Tools" },
    // Lighting
    CatalogApp { id: "grandma3", name: "grandMA3", category: "Lighting", win_process: "gma3.exe", mac_process: "gma3", linux_process: "gma3", win_install_hint: "MALightingTechnology", mac_install_hint: "grandMA3" },
    CatalogApp { id: "grandma2", name: "grandMA2", category: "Lighting", win_process: "gma2.exe", mac_process: "", linux_process: "", win_install_hint: "MALightingTechnology", mac_install_hint: "" },
    CatalogApp { id: "hog4", name: "Hog 4 PC", category: "Lighting", win_process: "Hog4PC.exe", mac_process: "", linux_process: "", win_install_hint: "Hog 4", mac_install_hint: "" },
    CatalogApp { id: "chamsys", name: "ChamSys MagicQ", category: "Lighting", win_process: "MagicQ.exe", mac_process: "MagicQ", linux_process: "MagicQ", win_install_hint: "ChamSys", mac_install_hint: "MagicQ" },
    CatalogApp { id: "eos", name: "ETC Eos Family", category: "Lighting", win_process: "Eos.exe", mac_process: "", linux_process: "", win_install_hint: "ETC", mac_install_hint: "" },
    CatalogApp { id: "capture", name: "Capture", category: "Lighting", win_process: "Capture.exe", mac_process: "Capture", linux_process: "", win_install_hint: "Capture", mac_install_hint: "Capture" },
    CatalogApp { id: "onyx", name: "ONYX (Obsidian)", category: "Lighting", win_process: "M-PC.exe", mac_process: "", linux_process: "", win_install_hint: "ONYX", mac_install_hint: "" },
    CatalogApp { id: "vista", name: "Chroma-Q Vista", category: "Lighting", win_process: "vista.exe", mac_process: "", linux_process: "", win_install_hint: "Vista", mac_install_hint: "" },
    CatalogApp { id: "lightkey", name: "Lightkey", category: "Lighting", win_process: "", mac_process: "Lightkey", linux_process: "", win_install_hint: "", mac_install_hint: "Lightkey" },
    CatalogApp { id: "qlcplus", name: "QLC+", category: "Lighting", win_process: "qlcplus.exe", mac_process: "qlcplus", linux_process: "qlcplus", win_install_hint: "QLC+", mac_install_hint: "QLC+" },
    // Playback
    CatalogApp { id: "qlab", name: "QLab", category: "Playback", win_process: "", mac_process: "QLab", linux_process: "", win_install_hint: "", mac_install_hint: "QLab" },
    CatalogApp { id: "playbackpro", name: "PlayBack Pro", category: "Playback", win_process: "PlayBackPro.exe", mac_process: "PlayBack Pro", linux_process: "", win_install_hint: "PlayBack Pro", mac_install_hint: "PlayBack Pro" },
];

/// Resolve a catalog app ID to the OS-specific process name.
pub fn resolve_process_name(app_id: &str, os: &str) -> Option<String> {
    PROTECTED_APPS_CATALOG.iter().find(|a| a.id == app_id).and_then(|a| {
        let name = match os {
            "windows" => a.win_process,
            "macos" | "darwin" => a.mac_process,
            "linux" => a.linux_process,
            _ => a.win_process, // default to windows (most common client OS)
        };
        if name.is_empty() { None } else { Some(name.to_string()) }
    })
}

/// Build the list of OS-specific process names for a given client OS,
/// combining catalog selections and custom process names.
pub fn resolve_protected_processes(
    enabled_ids: &[String],
    custom: &[String],
    os: &str,
) -> Vec<String> {
    let mut procs: Vec<String> = enabled_ids
        .iter()
        .filter_map(|id| resolve_process_name(id, os))
        .collect();
    procs.extend(custom.iter().cloned());
    procs
}

#[derive(Deserialize)]
pub struct SetProtectedAppsBody {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub custom_processes: Vec<String>,
}

/// GET /api/settings/protected-apps — return the full catalog + current selections.
pub async fn get_protected_apps(State(state): State<AppState>) -> Json<Value> {
    let enabled = state.inner.protected_apps.read().await;
    let custom = state.inner.custom_processes.read().await;

    // Build catalog JSON
    let catalog: Vec<Value> = PROTECTED_APPS_CATALOG
        .iter()
        .map(|a| json!({
            "id": a.id,
            "name": a.name,
            "category": a.category,
            "win_process": a.win_process,
            "mac_process": a.mac_process,
            "linux_process": a.linux_process,
        }))
        .collect();

    // Aggregate installed apps across all clients
    let clients = state.inner.clients.read().await;
    let mut installed_on: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    for client in clients.iter() {
        for app_id in &client.installed_apps {
            installed_on
                .entry(app_id.clone())
                .or_default()
                .push(json!({ "id": client.id, "hostname": &client.hostname }));
        }
    }

    Json(json!({
        "catalog": catalog,
        "enabled": *enabled,
        "custom_processes": *custom,
        "installed_on": installed_on,
    }))
}

/// PUT /api/settings/protected-apps — update the enabled app list + custom processes.
pub async fn set_protected_apps(
    State(state): State<AppState>,
    Json(body): Json<SetProtectedAppsBody>,
) -> Json<Value> {
    // Validate enabled IDs against catalog
    let valid_ids: Vec<&str> = PROTECTED_APPS_CATALOG.iter().map(|a| a.id).collect();
    let invalid: Vec<_> = body.enabled.iter()
        .filter(|id| !valid_ids.contains(&id.as_str()))
        .collect();
    if !invalid.is_empty() {
        return Json(json!({
            "success": false,
            "error": format!("Unknown app IDs: {}", invalid.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
        }));
    }

    // Persist to config file
    let config_path = state.inner.config_path.read().await.clone();
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(path = %config_path, error = %e, "Failed to read config file");
            return Json(json!({ "success": false, "error": format!("Cannot read config: {}", e) }));
        }
    };

    let mut table: toml::Table = match toml::from_str(&contents) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "Failed to parse config TOML");
            return Json(json!({ "success": false, "error": format!("Config parse error: {}", e) }));
        }
    };

    // Build [safety] section
    let safety = table
        .entry("safety")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(ref mut t) = safety {
        t.insert(
            "protected_apps".to_string(),
            toml::Value::Array(body.enabled.iter().map(|s| toml::Value::String(s.clone())).collect()),
        );
        t.insert(
            "custom_processes".to_string(),
            toml::Value::Array(body.custom_processes.iter().map(|s| toml::Value::String(s.clone())).collect()),
        );
    }

    let new_contents = match toml::to_string_pretty(&table) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({ "success": false, "error": format!("Failed to serialize config: {}", e) }));
        }
    };

    // Atomic write
    let tmp_path = format!("{}.tmp", config_path);
    if let Err(e) = std::fs::write(&tmp_path, &new_contents) {
        return Json(json!({ "success": false, "error": format!("Failed to write config: {}", e) }));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &config_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Json(json!({ "success": false, "error": format!("Failed to apply config: {}", e) }));
    }

    // Update in-memory state
    info!(
        enabled = ?body.enabled,
        custom = ?body.custom_processes,
        "Protected apps configuration updated"
    );
    *state.inner.protected_apps.write().await = body.enabled;
    *state.inner.custom_processes.write().await = body.custom_processes;

    Json(json!({ "success": true }))
}

