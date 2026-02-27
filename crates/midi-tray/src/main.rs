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
//
// On Windows, the Win32 message queue must be pumped with PeekMessageW
// for the tray icon context menu to work.

mod autostart;
mod icons;
mod menu;
mod process_manager;
mod ws_client;

use std::time::{Duration, Instant};

use muda::MenuEvent;
use tray_icon::TrayIconBuilder;
#[cfg(target_os = "windows")]
use tracing::error;
use tracing::info;

use midi_protocol::health::{ClientHealthSnapshot, ConnectionState, TrayCommand};

use crate::icons::{color_for_snapshot, IconCache, IconColor};
use crate::menu::{
    build_disconnected_menu, build_initial_menu, build_status_menu, MenuState, ID_AUTO_START,
    ID_CLAIM_FOCUS, ID_OPEN_DASHBOARD, ID_QUIT, ID_RELEASE_FOCUS, ID_RESTART_CLIENT,
};
#[cfg(target_os = "windows")]
use crate::process_manager::ProcessStatus;
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

// ── Windows: atomic flag for system shutdown/logoff signals ──
#[cfg(target_os = "windows")]
static SHUTDOWN_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Show a native "Cannot Quit" dialog when Resolume Arena is detected.
#[cfg(target_os = "windows")]
fn show_resolume_block_dialog() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let text: Vec<u16> = OsStr::new(
        "Resolume Arena is currently using MIDInet.\n\n\
         To safely quit, please close Resolume Arena first,\n\
         then select Quit again from the tray menu.",
    )
    .encode_wide()
    .chain(Some(0))
    .collect();
    let caption: Vec<u16> = OsStr::new("MIDInet")
        .encode_wide()
        .chain(Some(0))
        .collect();

    // MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            0x00000000 | 0x00000010 | 0x00040000 | 0x00010000,
        );
    }
}

/// Show a strict confirmation dialog on Windows. Returns true if user clicked Yes.
#[cfg(target_os = "windows")]
fn confirm_quit() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let text: Vec<u16> = OsStr::new(
        "Quitting will disconnect the virtual MIDI device.\n\n\
         Any application using this device will lose its MIDI connection.\n\n\
         Would you like to continue?",
    )
    .encode_wide()
    .chain(Some(0))
    .collect();
    let caption: Vec<u16> = OsStr::new("Quit MIDInet")
        .encode_wide()
        .chain(Some(0))
        .collect();

    // MB_YESNO | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND | MB_DEFBUTTON2
    let result = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            0x00000004 | 0x00000030 | 0x00040000 | 0x00010000 | 0x00000100,
        )
    };
    result == 6 // IDYES
}

fn main() {
    // ── Panic hook: log + show dialog on crash ──
    // Must be first — catches panics in all subsequent initialization.
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                info.payload()
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
            })
            .unwrap_or("unknown panic");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let msg = format!("MIDInet tray panicked at {}: {}", location, payload);

        // Write to panic log file next to the executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let log_dir = dir.join("logs");
                let _ = std::fs::create_dir_all(&log_dir);
                let path = log_dir.join("tray-panic.log");
                let _ = std::fs::write(&path, &msg);
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            let text: Vec<u16> = OsStr::new(&msg).encode_wide().chain(Some(0)).collect();
            let cap: Vec<u16> = OsStr::new("MIDInet Crash")
                .encode_wide()
                .chain(Some(0))
                .collect();
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    cap.as_ptr(),
                    0x00000010 | 0x00040000, // MB_ICONERROR | MB_TOPMOST
                );
            }
        }
    }));

    // ── Single-instance guard (Windows) ──
    // Prevents duplicate tray instances from fighting over the same client.
    // Opens a lock file with share_mode(0) = exclusive access. If another
    // instance already holds it, the open fails and we exit.
    #[cfg(target_os = "windows")]
    let _instance_lock = {
        use std::os::windows::fs::OpenOptionsExt;
        let lock_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("logs")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&lock_dir);
        let lock_path = lock_dir.join(".tray.lock");
        match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0) // exclusive — no other process can open
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(_) => {
                // Another instance holds the lock
                std::process::exit(0);
            }
        }
    };

    // ── File-based logging ──
    // On Windows with windows_subsystem="windows", stderr is /dev/null.
    // Write to a rotating daily log file next to the executable.
    let log_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("logs")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "tray.log");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    info!("MIDInet tray starting");

    // ── Windows: console ctrl handler for graceful shutdown on logoff/shutdown ──
    #[cfg(target_os = "windows")]
    {
        unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
            // CTRL_CLOSE_EVENT=2, CTRL_LOGOFF_EVENT=5, CTRL_SHUTDOWN_EVENT=6
            if ctrl_type == 2 || ctrl_type == 5 || ctrl_type == 6 {
                SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                return 1; // handled
            }
            0
        }

        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 1);
        }
    }

    // ── macOS: initialize NSApplication with Accessory policy ──
    // This ensures the app appears as a menu bar item only (no dock icon).
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
                        std::env::var("LOCALAPPDATA")
                            .ok()
                            .map(|appdata| {
                                std::path::PathBuf::from(appdata)
                                    .join("MIDInet")
                                    .join("config")
                                    .join("client.toml")
                            })
                            .filter(|p| p.exists())
                    });
                let mut mgr = ProcessManager::new(client_path, config_path);
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

    // Pre-generate all icon variants (no runtime pixel computation)
    let icon_cache = IconCache::new();

    // Build initial tray icon (gray = daemon not yet connected)
    let initial_icon = icon_cache.get(IconColor::Gray, false);
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

    // Blink state: green icon pulses between bright and dim (~480ms per phase)
    // Uses wall-clock timing so blink works regardless of loop sleep duration.
    let mut blink_on = true;
    let mut last_blink_toggle = Instant::now();
    const BLINK_INTERVAL: Duration = Duration::from_millis(480);

    // Process monitoring counter (check every ~500ms)
    #[cfg(target_os = "windows")]
    let mut last_proc_check = Instant::now();

    // Menu diffing state — only rebuild when snapshot fields change
    let mut last_menu_state: Option<MenuState> = None;
    #[allow(unused_mut)]
    let mut auto_start_cached: bool = autostart::is_enabled();

    // Notification cooldown to prevent spam during flapping
    let mut last_notification: Option<Instant> = None;
    const NOTIFICATION_COOLDOWN: Duration = Duration::from_secs(5);

    let menu_rx = MenuEvent::receiver();

    info!("Tray running -- right-click the icon for status");

    'main: loop {
        // ── Check for system shutdown signal (Windows) ──
        #[cfg(target_os = "windows")]
        if SHUTDOWN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            info!("System shutdown/logoff signal received");
            proc_mgr.graceful_shutdown(Duration::from_secs(3));
            break 'main;
        }

        // ── Process WebSocket events (non-blocking) ──
        while let Ok(event) = ws_rx.try_recv() {
            match event {
                WsEvent::Snapshot(snapshot) => {
                    let new_color = color_for_snapshot(&snapshot);
                    if new_color != current_color {
                        let icon = icon_cache.get(new_color, false);
                        let _ = tray.set_icon(Some(icon));
                        current_color = new_color;
                        blink_on = true;
                        last_blink_toggle = Instant::now();
                    }

                    // Update tooltip
                    let tooltip = format_tooltip(&snapshot);
                    let _ = tray.set_tooltip(Some(&tooltip));

                    // Update menu only if state changed
                    let new_menu_state =
                        MenuState::from_snapshot(&snapshot, auto_start_cached);
                    if last_menu_state.as_ref() != Some(&new_menu_state) {
                        let menu_obj =
                            build_status_menu(&snapshot, auto_start_cached);
                        let _ = tray.set_menu(Some(Box::new(menu_obj)));
                        last_menu_state = Some(new_menu_state);
                    }

                    // Check for failover notifications (with cooldown)
                    if let Some(ref prev) = last_snapshot {
                        let should_notify = last_notification
                            .map_or(true, |t| t.elapsed() >= NOTIFICATION_COOLDOWN);
                        if should_notify && check_notifications(prev, &snapshot) {
                            last_notification = Some(Instant::now());
                        }
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
                        last_menu_state = None;

                        let icon = icon_cache.get(IconColor::Gray, false);
                        let _ = tray.set_icon(Some(icon));
                        current_color = IconColor::Gray;
                        blink_on = true;
                        last_blink_toggle = Instant::now();

                        let _ = tray.set_tooltip(Some("MIDInet: Daemon not running"));
                        let menu = build_disconnected_menu();
                        let _ = tray.set_menu(Some(Box::new(menu)));
                    }
                }
            }
        }

        // ── Blink the icon when connected (green) — wall-clock based ──
        if current_color == IconColor::Green
            && last_blink_toggle.elapsed() >= BLINK_INTERVAL
        {
            last_blink_toggle = Instant::now();
            blink_on = !blink_on;
            let icon = icon_cache.get(IconColor::Green, !blink_on);
            let _ = tray.set_icon(Some(icon));
        }

        // ── Windows: monitor child process (~every 500ms) ──
        #[cfg(target_os = "windows")]
        {
            if last_proc_check.elapsed() >= Duration::from_millis(500) {
                last_proc_check = Instant::now();
                proc_mgr.reset_backoff();

                match proc_mgr.check() {
                    ProcessStatus::Running => {} // healthy
                    ProcessStatus::NotStarted => {
                        // Initial spawn failed — retry
                        if proc_mgr.should_restart() {
                            if let Err(e) = proc_mgr.spawn() {
                                error!("Failed to spawn client: {}", e);
                            }
                        }
                    }
                    ProcessStatus::Exited(code) => match code {
                        Some(0) => {
                            info!("Client exited cleanly (code 0)");
                        }
                        _ => {
                            if proc_mgr.should_restart() {
                                if let Err(e) = proc_mgr.restart() {
                                    error!("Failed to restart client: {}", e);
                                }
                            }
                        }
                    },
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
                        if process_manager::is_resolume_running() {
                            show_resolume_block_dialog();
                            continue;
                        }
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
                                auto_start_cached = enabled;
                                // Force menu rebuild to reflect new state
                                last_menu_state = None;
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
                        // Block quit if Resolume Arena is running
                        if process_manager::is_resolume_running() {
                            show_resolume_block_dialog();
                            continue;
                        }
                        if !confirm_quit() {
                            continue;
                        }
                        info!("Shutting down client and exiting");
                        proc_mgr.graceful_shutdown(Duration::from_secs(5));
                        break 'main;
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        info!("Quit requested");
                        break 'main;
                    }
                }
                _ => {}
            }
        }

        // ── Pump the event loop ──
        // On macOS: pump the native CFRunLoop for menu bar icon to render.
        // On Windows: pump the Win32 message queue for right-click context menu.
        // On Linux: a simple sleep suffices (GTK loop is implicit via tray-icon).
        #[cfg(target_os = "macos")]
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.016, 0);
        }
        #[cfg(target_os = "windows")]
        {
            unsafe {
                let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG =
                    std::mem::zeroed();
                while windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
                    &mut msg,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0x0001, // PM_REMOVE
                ) != 0
                {
                    windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            }
            // Adaptive sleep: faster during blink animation, slower when idle
            let sleep_ms = if current_color == IconColor::Green { 16 } else { 100 };
            std::thread::sleep(Duration::from_millis(sleep_ms));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        std::thread::sleep(Duration::from_millis(100));
    }

    // ── Clean shutdown ──
    // Explicit drop order: tray icon first (removes from system tray),
    // then ProcessManager (graceful client shutdown), then log guard (flush).
    info!("MIDInet tray shutting down");
    drop(tray);
    // proc_mgr dropped here (Windows), _log_guard dropped last
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
        "MIDInet {} | {} | {:.0} in {:.0} out msg/s | {:.1}% loss",
        midi_protocol::version_string(),
        state,
        snapshot.midi_rate_in,
        snapshot.midi_rate_out,
        snapshot.packet_loss_percent
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
/// significant state transitions. Returns true if a notification was shown.
fn check_notifications(prev: &ClientHealthSnapshot, curr: &ClientHealthSnapshot) -> bool {
    // Failover occurred
    if curr.failover_count > prev.failover_count {
        let host_name = curr
            .active_host
            .as_ref()
            .map(|h| capitalize(&h.role))
            .unwrap_or_else(|| "unknown".to_string());
        show_notification("MIDInet Failover", &format!("Switched to {} host", host_name));
        return true;
    }

    // Both hosts lost
    if prev.connection_state != ConnectionState::Disconnected
        && curr.connection_state == ConnectionState::Disconnected
    {
        show_notification("MIDInet", "All hosts unreachable!");
        return true;
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
        return true;
    }

    false
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
