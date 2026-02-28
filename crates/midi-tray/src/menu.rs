/// Context menu for the system tray.
///
/// Displays connection status, metrics, and actions (focus, dashboard, quit).
/// Menus are only rebuilt when the underlying state changes (see `MenuState`).

use muda::accelerator::Accelerator;
use muda::{Menu, MenuItem, PredefinedMenuItem};

use midi_protocol::health::{ClientHealthSnapshot, ConnectionState};

/// Identifiers for menu items that trigger actions.
pub const ID_CLAIM_FOCUS: &str = "claim_focus";
pub const ID_RELEASE_FOCUS: &str = "release_focus";
pub const ID_OPEN_DASHBOARD: &str = "open_dashboard";
pub const ID_RESTART_CLIENT: &str = "restart_client";
pub const ID_AUTO_START: &str = "auto_start";
pub const ID_CHECK_UPDATE: &str = "check_update";
pub const ID_QUIT: &str = "quit";

/// Snapshot of menu-driving state for diffing. Menu is only rebuilt when this changes.
#[derive(PartialEq, Eq)]
pub struct MenuState {
    pub connection_state: ConnectionState,
    pub active_host_role: Option<String>,
    pub midi_rate_in: u32,
    pub midi_rate_out: u32,
    pub packet_loss_tenth: u32,
    pub has_focus: bool,
    pub hosts_discovered: u8,
    pub uptime_mins: u64,
    pub has_dashboard: bool,
    pub auto_start: bool,
    pub version_mismatch: bool,
}

impl MenuState {
    pub fn from_snapshot(snapshot: &ClientHealthSnapshot, auto_start: bool) -> Self {
        Self {
            connection_state: snapshot.connection_state,
            active_host_role: snapshot.active_host.as_ref().map(|h| h.role.clone()),
            midi_rate_in: snapshot.midi_rate_in as u32,
            midi_rate_out: snapshot.midi_rate_out as u32,
            packet_loss_tenth: (snapshot.packet_loss_percent * 10.0) as u32,
            has_focus: snapshot.has_focus,
            hosts_discovered: snapshot.hosts_discovered,
            uptime_mins: snapshot.uptime_secs / 60,
            has_dashboard: snapshot.admin_url.is_some(),
            auto_start,
            version_mismatch: snapshot.version_mismatch,
        }
    }
}

/// Build the initial tray context menu (before any daemon connection).
pub fn build_initial_menu() -> Menu {
    let menu = Menu::new();

    let _ = menu.append(&MenuItem::with_id(
        "status_line",
        "MIDInet: Not connected",
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(
        "info_line",
        "Waiting for daemon...",
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    // Check for updates
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_CHECK_UPDATE,
            "Check for Updates",
            true,
            None::<Accelerator>,
        ));
    }

    // Restart client
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_RESTART_CLIENT,
            "Restart Client",
            true,
            None::<Accelerator>,
        ));
    }

    // Auto-start toggle
    #[cfg(target_os = "windows")]
    {
        let auto_label = if crate::autostart::is_enabled() {
            "Start with Windows  [ON]"
        } else {
            "Start with Windows  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let auto_label = if crate::autostart::is_enabled() {
            "Start at Login  [ON]"
        } else {
            "Start at Login  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(
        "version_line",
        &format!("MIDInet {}", midi_protocol::version_string()),
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit MIDInet", true, None::<Accelerator>));

    menu
}

/// Build an updated menu reflecting the current health snapshot.
pub fn build_status_menu(snapshot: &ClientHealthSnapshot, auto_start: bool) -> Menu {
    let menu = Menu::new();

    // ── Status line ──
    let status_text = match snapshot.connection_state {
        ConnectionState::Connected => {
            let role = snapshot
                .active_host
                .as_ref()
                .map(|h| h.role.as_str())
                .unwrap_or("unknown");
            format!("Connected to {}", capitalize(role))
        }
        ConnectionState::Discovering => "Discovering hosts...".to_string(),
        ConnectionState::Reconnecting => "Reconnecting...".to_string(),
        ConnectionState::Disconnected => "Disconnected".to_string(),
    };
    let _ = menu.append(&MenuItem::with_id("status_line", &status_text, false, None::<Accelerator>));

    // ── Metrics ──
    let _ = menu.append(&MenuItem::with_id(
        "rate_line",
        &format!(
            "{:.0} msg/s in | {:.0} msg/s out",
            snapshot.midi_rate_in, snapshot.midi_rate_out
        ),
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&MenuItem::with_id(
        "loss_line",
        &format!("Packet loss: {:.1}%", snapshot.packet_loss_percent),
        false,
        None::<Accelerator>,
    ));

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── Focus & Hosts ──
    let focus_text = if snapshot.has_focus {
        "Focus: Claimed"
    } else {
        "Focus: Not held"
    };
    let _ = menu.append(&MenuItem::with_id("focus_line", focus_text, false, None::<Accelerator>));
    let _ = menu.append(&MenuItem::with_id(
        "hosts_line",
        &format!("Hosts: {} discovered", snapshot.hosts_discovered),
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&MenuItem::with_id(
        "uptime_line",
        &format!("Uptime: {}", format_duration(snapshot.uptime_secs)),
        false,
        None::<Accelerator>,
    ));

    let _ = menu.append(&PredefinedMenuItem::separator());

    // ── Actions ──
    let _ = menu.append(&MenuItem::with_id(
        ID_CLAIM_FOCUS,
        "Claim Focus",
        !snapshot.has_focus,
        None::<Accelerator>,
    ));
    let _ = menu.append(&MenuItem::with_id(
        ID_RELEASE_FOCUS,
        "Release Focus",
        snapshot.has_focus,
        None::<Accelerator>,
    ));

    let _ = menu.append(&PredefinedMenuItem::separator());

    let has_dashboard = snapshot.admin_url.is_some();
    let _ = menu.append(&MenuItem::with_id(
        ID_OPEN_DASHBOARD,
        "Open Admin Dashboard",
        has_dashboard,
        None::<Accelerator>,
    ));

    let _ = menu.append(&PredefinedMenuItem::separator());

    // Version mismatch warning — directional guidance
    if snapshot.version_mismatch {
        // The client's compiled hash is always client_git_hash.
        // If the host hash is different, the host is the one that needs updating
        // (the user already has the latest client binary running).
        let client_is_current = snapshot.client_git_hash == midi_protocol::GIT_HASH;
        let warn_text = if client_is_current {
            "!! Host outdated — update via dashboard or SSH"
        } else {
            "!! Client outdated — check for updates"
        };
        let _ = menu.append(&MenuItem::with_id(
            "mismatch_warn",
            warn_text,
            false,
            None::<Accelerator>,
        ));
        let _ = menu.append(&MenuItem::with_id(
            "mismatch_detail",
            &format!(
                "Host: {} | Client: {}",
                snapshot.host_git_hash, snapshot.client_git_hash
            ),
            false,
            None::<Accelerator>,
        ));
        let _ = menu.append(&PredefinedMenuItem::separator());
    }

    // Check for updates
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_CHECK_UPDATE,
            "Check for Updates",
            true,
            None::<Accelerator>,
        ));
    }

    // Restart client
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_RESTART_CLIENT,
            "Restart Client",
            true,
            None::<Accelerator>,
        ));
    }

    // Auto-start toggle
    #[cfg(target_os = "windows")]
    {
        let auto_label = if auto_start {
            "Start with Windows  [ON]"
        } else {
            "Start with Windows  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let auto_label = if auto_start {
            "Start at Login  [ON]"
        } else {
            "Start at Login  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(
        "version_line",
        &format!("MIDInet {}", midi_protocol::version_string()),
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit MIDInet", true, None::<Accelerator>));

    menu
}

/// Build a menu shown when the daemon is unreachable.
pub fn build_disconnected_menu() -> Menu {
    let menu = Menu::new();

    let _ = menu.append(&MenuItem::with_id(
        "status_line",
        "Daemon not running",
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(
        "info_line",
        "Waiting for midinet-client...",
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    // Check for updates
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_CHECK_UPDATE,
            "Check for Updates",
            true,
            None::<Accelerator>,
        ));
    }

    // Restart client
    {
        let _ = menu.append(&MenuItem::with_id(
            ID_RESTART_CLIENT,
            "Restart Client",
            true,
            None::<Accelerator>,
        ));
    }

    // Auto-start toggle
    #[cfg(target_os = "windows")]
    {
        let auto_label = if crate::autostart::is_enabled() {
            "Start with Windows  [ON]"
        } else {
            "Start with Windows  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let auto_label = if crate::autostart::is_enabled() {
            "Start at Login  [ON]"
        } else {
            "Start at Login  [OFF]"
        };
        let _ = menu.append(&MenuItem::with_id(
            ID_AUTO_START,
            auto_label,
            true,
            None::<Accelerator>,
        ));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(
        "version_line",
        &format!("MIDInet {}", midi_protocol::version_string()),
        false,
        None::<Accelerator>,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit MIDInet", true, None::<Accelerator>));

    menu
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}
