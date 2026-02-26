// Hide console window on Windows (tray is a GUI app)
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

// MIDInet system tray application.
//
// Displays a color-coded icon (green/yellow/red/gray) reflecting the
// client daemon's health. Right-click for status details and actions.
//
// Architecture:
//   - Main thread: native GUI event loop (required by tray-icon)
//   - Background thread: WebSocket client to ws://127.0.0.1:5009/ws
//   - On Windows: spawns midi-client.exe as a hidden child process
//
// On macOS, the native NSApplication run loop must be pumped for the
// menu bar icon to render. We use CFRunLoopRunInMode instead of
// std::thread::sleep to drive the event loop.

mod autostart;
mod icons;
mod menu;
mod process_manager;
mod ws_client;

#[cfg(not(target_os = "macos"))]
use std::time::Duration;

use muda::MenuEvent;
use tray_icon::TrayIconBuilder;
#[cfg(target_os = "windows")]
use tracing::error;
use tracing::info;

use midi_protocol::health::{ClientHealthSnapshot, ConnectionState, TrayCommand};

use crate::icons::{color_for_snapshot, generate_icon, generate_icon_dim, IconColor};
use crate::menu::{
    build_disconnected_menu, build_initial_menu, build_status_menu, ID_CLAIM_FOCUS,
    ID_OPEN_DASHBOARD, ID_QUIT, ID_RELEASE_FOCUS, ID_RESTART_CLIENT, ID_AUTO_START,
};
use crate::ws_client::{send_command, spawn_ws_thread, WsEvent};

// ── macOS: raw FFI for CoreFoundation run loop ──
#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    fn CFRunLoopRunInMode(
        mode: *const std::ffi::c_void,
        seconds: f64,
        return_after_source_handled: u8,
    ) -> i32;
}

/// Show a native confirmation dialog on Windows. Returns true if user clicked Yes.
#[cfg(target_os = "windows")]
fn confirm_quit() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let text: Vec<u16> = OsStr::new(
        "This will stop the MIDI client and disconnect virtual devices.\n\nRunning applications that use the MIDI device may crash.\n\nAre you sure?",
    )
    .encode_wide()
    .chain(Some(0))
    .collect();
    let caption: Vec<u16> = OsStr::new("Quit MIDInet")
        .encode_wide()
        .chain(Some(0))
        .collect();

    // MB_YESNO | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND
    let result = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            0x00000004 | 0x00000030 | 0x00040000 | 0x00010000,
        )
    };
    result == 6 // IDYES
}

fn main() {
    // Set up logging — on Windows with windows_subsystem="windows", stderr goes
    // nowhere, but tracing is still useful for file-based logging or debuggers.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("MIDInet tray starting");

    // ── macOS: initialize NSApplication with Accessory policy ──
    // This ensures the app appears as a menu bar item only (no dock icon).
    // Must be done before creating the tray icon.
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

        let mtm = MainThreadMarker::new().expect("must be called from main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        info!("macOS: NSApplication initialized with Accessory policy");
    }

    // ── Windows: spawn midi-client as a hidden child process ──
    #[cfg(target_os = "windows")]
    let mut proc_mgr = {
        use crate::process_manager::{find_client_binary, ProcessManager};
        match find_client_binary() {
            Some(client_path) => {
                // Look for config: alongside binary first, then in %LOCALAPPDATA%\MIDInet\config\
                let config_path = client_path
                    .parent()
                    .map(|d| d.join("config").join("client.toml"))
                    .filter(|p| p.exists())
                    .or_else(|| {
                        std::env::var("LOCALAPPDATA").ok().map(|appdata| {
                            std::path::PathBuf::from(appdata)
                                .join("MIDInet")
                                .join("config")
                                .join("client.toml")
                        }).filter(|p| p.exists())
                    });
                let mut mgr = ProcessManager::new(client_path, config_path);
                // Kill any existing client instances (old scheduled task, manual runs, etc.)
                mgr.kill_existing_clients();
                if let Err(e) = mgr.spawn() {
                    error!("Failed to spawn midi-client: {}", e);
                }
                mgr
            }
            None => {
                error!("midi-client not found. Place it in the same directory as midi-tray.");
                ProcessManager::new(std::path::PathBuf::from("midi-client"), None)
            }
        }
    };

    // Start the WebSocket background thread
    let ws_rx = spawn_ws_thread();

    // Build initial tray icon (gray = daemon not yet connected)
    let initial_icon = generate_icon(IconColor::Gray);
    let initial_menu = build_initial_menu();

    let tray = TrayIconBuilder::new()
        .with_icon(initial_icon)
        .with_tooltip("MIDInet: Starting...")
        .with_menu(Box::new(initial_menu))
        .build()
        .expect("failed to build tray icon");

    let mut current_color = IconColor::Gray;
    let mut last_snapshot: Option<ClientHealthSnapshot> = None;
    let mut daemon_connected = false;

    // Blink state: green icon pulses between bright and dim (~1s cycle)
    let mut blink_tick: u32 = 0;
    let mut blink_on = true;
    const BLINK_HALF_PERIOD: u32 = 30; // 30 ticks * 16ms ~ 480ms per phase

    // Process monitoring counter (check every ~500ms = 30 ticks at 16ms)
    #[cfg(target_os = "windows")]
    let mut proc_check_tick: u32 = 0;
    #[cfg(target_os = "windows")]
    let mut quit_requested = false;

    let menu_rx = MenuEvent::receiver();

    info!("Tray running -- right-click the icon for status");

    loop {
        // ── Process WebSocket events (non-blocking) ──
        while let Ok(event) = ws_rx.try_recv() {
            match event {
                WsEvent::Snapshot(snapshot) => {
                    let new_color = color_for_snapshot(&snapshot);
                    if new_color != current_color {
                        let icon = generate_icon(new_color);
                        let _ = tray.set_icon(Some(icon));
                        current_color = new_color;
                        blink_on = true;
                        blink_tick = 0;
                    }

                    // Update tooltip
                    let tooltip = format_tooltip(&snapshot);
                    let _ = tray.set_tooltip(Some(&tooltip));

                    // Update menu
                    let menu = build_status_menu(&snapshot);
                    let _ = tray.set_menu(Some(Box::new(menu)));

                    // Check for failover notifications
                    if let Some(ref prev) = last_snapshot {
                        check_notifications(prev, &snapshot);
                    }

                    last_snapshot = Some(snapshot);
                    daemon_connected = true;
                }
                WsEvent::Connected => {
                    daemon_connected = true;
                    info!("Connected to daemon");
                }
                WsEvent::Disconnected => {
                    if daemon_connected {
                        info!("Lost connection to daemon");
                        daemon_connected = false;
                        last_snapshot = None;

                        let icon = generate_icon(IconColor::Gray);
                        let _ = tray.set_icon(Some(icon));
                        current_color = IconColor::Gray;
                        blink_on = true;
                        blink_tick = 0;

                        let _ = tray.set_tooltip(Some("MIDInet: Daemon not running"));
                        let menu = build_disconnected_menu();
                        let _ = tray.set_menu(Some(Box::new(menu)));
                    }
                }
            }
        }

        // ── Blink the icon when connected (green) ──
        if current_color == IconColor::Green {
            blink_tick += 1;
            if blink_tick >= BLINK_HALF_PERIOD {
                blink_tick = 0;
                blink_on = !blink_on;
                let icon = if blink_on {
                    generate_icon(IconColor::Green)
                } else {
                    generate_icon_dim(IconColor::Green)
                };
                let _ = tray.set_icon(Some(icon));
            }
        }

        // ── Windows: monitor child process ──
        #[cfg(target_os = "windows")]
        {
            proc_check_tick += 1;
            if proc_check_tick >= 30 {
                proc_check_tick = 0;
                proc_mgr.reset_backoff();

                if let Some(exit_code) = proc_mgr.check() {
                    if quit_requested {
                        // Intentional quit -- exit the tray
                        std::process::exit(0);
                    }
                    match exit_code {
                        Some(0) => {
                            // Client exited cleanly on its own
                            info!("Client exited cleanly (code 0)");
                        }
                        _ => {
                            // Crash or signal -- auto-restart with backoff
                            if proc_mgr.should_restart() {
                                if let Err(e) = proc_mgr.restart() {
                                    error!("Failed to restart client: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── Process menu events (non-blocking) ──
        while let Ok(event) = menu_rx.try_recv() {
            let id = event.id().0.as_str();
            match id {
                ID_CLAIM_FOCUS => {
                    send_command(&TrayCommand::ClaimFocus);
                }
                ID_RELEASE_FOCUS => {
                    send_command(&TrayCommand::ReleaseFocus);
                }
                ID_OPEN_DASHBOARD => {
                    if let Some(ref snap) = last_snapshot {
                        if let Some(ref url) = snap.admin_url {
                            let _ = open::that(url);
                        }
                    }
                }
                ID_RESTART_CLIENT => {
                    #[cfg(target_os = "windows")]
                    {
                        info!("Restart client requested via menu");
                        proc_mgr.graceful_shutdown(Duration::from_secs(5));
                        if let Err(e) = proc_mgr.spawn() {
                            error!("Failed to restart client: {}", e);
                        }
                    }
                }
                ID_AUTO_START => {
                    #[cfg(target_os = "windows")]
                    {
                        match autostart::toggle() {
                            Ok(enabled) => {
                                info!(enabled, "Auto-start toggled");
                            }
                            Err(e) => {
                                error!("Failed to toggle auto-start: {}", e);
                            }
                        }
                    }
                }
                ID_QUIT => {
                    #[cfg(target_os = "windows")]
                    {
                        if !confirm_quit() {
                            continue;
                        }
                        quit_requested = true;
                        info!("Shutting down client and exiting");
                        proc_mgr.graceful_shutdown(Duration::from_secs(5));
                        std::process::exit(0);
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        info!("Quit requested");
                        std::process::exit(0);
                    }
                }
                _ => {}
            }
        }

        // ── Pump the event loop ──
        // On macOS, we must pump the native CFRunLoop for the menu bar icon
        // to render. On other platforms, a simple sleep suffices.
        #[cfg(target_os = "macos")]
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.016, 0);
        }
        #[cfg(not(target_os = "macos"))]
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn format_tooltip(snapshot: &ClientHealthSnapshot) -> String {
    let state = match snapshot.connection_state {
        ConnectionState::Connected => {
            let role = snapshot
                .active_host
                .as_ref()
                .map(|h| capitalize(&h.role))
                .unwrap_or_else(|| "?".to_string());
            if snapshot.device_ready && !snapshot.device_name.is_empty() {
                format!("{} | {}", role, snapshot.device_name)
            } else {
                format!("{} | No MIDI device", role)
            }
        }
        ConnectionState::Discovering => "Discovering hosts...".to_string(),
        ConnectionState::Reconnecting => "Reconnecting...".to_string(),
        ConnectionState::Disconnected => "Disconnected".to_string(),
    };

    format!(
        "MIDInet: {} | {:.0} msg/s | {:.1}% loss",
        state, snapshot.midi_rate_in, snapshot.packet_loss_percent
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Compare consecutive snapshots and show desktop notifications for
/// significant state transitions.
fn check_notifications(prev: &ClientHealthSnapshot, curr: &ClientHealthSnapshot) {
    // Failover occurred
    if curr.failover_count > prev.failover_count {
        let host_name = curr
            .active_host
            .as_ref()
            .map(|h| capitalize(&h.role))
            .unwrap_or_else(|| "unknown".to_string());
        show_notification("MIDInet Failover", &format!("Switched to {} host", host_name));
    }

    // Both hosts lost
    if prev.connection_state != ConnectionState::Disconnected
        && curr.connection_state == ConnectionState::Disconnected
    {
        show_notification("MIDInet", "All hosts unreachable!");
    }

    // Reconnected after outage
    if prev.connection_state == ConnectionState::Reconnecting
        && curr.connection_state == ConnectionState::Connected
    {
        let host_name = curr
            .active_host
            .as_ref()
            .map(|h| capitalize(&h.role))
            .unwrap_or_else(|| "host".to_string());
        show_notification("MIDInet", &format!("Reconnected to {}", host_name));
    }
}

fn show_notification(title: &str, body: &str) {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    }
}
