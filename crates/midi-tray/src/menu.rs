/// Context menu for the system tray.
///
/// Displays connection status, metrics, and actions (focus, dashboard, quit).

use muda::accelerator::Accelerator;
use muda::{Menu, MenuItem, PredefinedMenuItem};

use midi_protocol::health::{ClientHealthSnapshot, ConnectionState};

/// Identifiers for menu items that trigger actions.
pub const ID_CLAIM_FOCUS: &str = "claim_focus";
pub const ID_RELEASE_FOCUS: &str = "release_focus";
pub const ID_OPEN_DASHBOARD: &str = "open_dashboard";
pub const ID_QUIT: &str = "quit";

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
    let _ = menu.append(&MenuItem::with_id(
        ID_QUIT,
        "Quit Tray",
        true,
        None::<Accelerator>,
    ));

    menu
}

/// Build an updated menu reflecting the current health snapshot.
pub fn build_status_menu(snapshot: &ClientHealthSnapshot) -> Menu {
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

    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit Tray", true, None::<Accelerator>));

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
    let _ = menu.append(&MenuItem::with_id(ID_QUIT, "Quit Tray", true, None::<Accelerator>));

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
