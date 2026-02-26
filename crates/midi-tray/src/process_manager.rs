// Most of this module is Windows-only but must compile on all platforms.
#![allow(dead_code)]

/// Child process lifecycle manager for midi-client on Windows.
///
/// Spawns midi-client.exe with CREATE_NO_WINDOW flag (no console window),
/// monitors the process, and auto-restarts on crash with exponential backoff.
/// Provides graceful shutdown via the client's /shutdown HTTP endpoint.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Result of checking the child process state.
pub enum ProcessStatus {
    /// Child is running normally
    Running,
    /// Child exited with the given code (None = signal/unknown)
    Exited(Option<i32>),
    /// No child process has been spawned or tracked
    NotStarted,
}

pub struct ProcessManager {
    child: Option<Child>,
    client_path: PathBuf,
    config_path: Option<PathBuf>,
    last_exit: Option<Instant>,
    restart_count: u32,
}

impl ProcessManager {
    pub fn new(client_path: PathBuf, config_path: Option<PathBuf>) -> Self {
        Self {
            child: None,
            client_path,
            config_path,
            last_exit: None,
            restart_count: 0,
        }
    }

    /// Kill any existing midi-client processes before spawning our own.
    /// This prevents conflicts when an old scheduled task or manual instance is running.
    pub fn kill_existing_clients(&self) {
        // First try graceful shutdown via the health API (with connect timeout)
        let addr: std::net::SocketAddr = format!(
            "127.0.0.1:{}",
            midi_protocol::health::DEFAULT_HEALTH_PORT
        )
        .parse()
        .unwrap();

        if let Ok(mut stream) =
            std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        {
            use std::io::Write;
            let request = "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(request.as_bytes());
            info!("Sent shutdown to existing client on health port");
            // Give it a moment to exit
            std::thread::sleep(Duration::from_secs(2));
        }

        // Force-kill any remaining midi-client / midinet-client processes
        #[cfg(target_os = "windows")]
        {
            let username = std::env::var("USERNAME").unwrap_or_default();
            for name in &["midi-client.exe", "midinet-client.exe"] {
                let mut args = vec!["/F", "/IM", *name];
                // Only filter by username if we know it (avoids killing other RDP users)
                let filter;
                if !username.is_empty() {
                    filter = format!("USERNAME eq {}", username);
                    args.extend(["/FI", &filter]);
                }
                let _ = Command::new("taskkill")
                    .args(&args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
            }
            // Brief pause for handles to release
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Spawn the client process (hidden on Windows, normal on other platforms).
    pub fn spawn(&mut self) -> Result<(), std::io::Error> {
        let mut cmd = Command::new(&self.client_path);
        if let Some(ref config) = self.config_path {
            cmd.args(["-c", &config.to_string_lossy()]);
        }
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let child = cmd.spawn()?;
        info!(pid = child.id(), path = %self.client_path.display(), "Spawned midi-client");
        self.child = Some(child);
        Ok(())
    }

    /// Check if the child is still running.
    pub fn check(&mut self) -> ProcessStatus {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    info!(exit_code = ?code, "midi-client exited");
                    self.child = None;
                    self.last_exit = Some(Instant::now());
                    ProcessStatus::Exited(code)
                }
                Ok(None) => ProcessStatus::Running,
                Err(e) => {
                    error!("Error checking client process: {}", e);
                    self.child = None;
                    ProcessStatus::Exited(None)
                }
            }
        } else {
            ProcessStatus::NotStarted
        }
    }

    /// Whether we should auto-restart based on backoff timing.
    pub fn should_restart(&self) -> bool {
        if let Some(last) = self.last_exit {
            let backoff_secs = std::cmp::min(self.restart_count as u64 * 2, 30).max(2);
            last.elapsed() >= Duration::from_secs(backoff_secs)
        } else {
            true
        }
    }

    /// Restart the client process.
    pub fn restart(&mut self) -> Result<(), std::io::Error> {
        self.restart_count += 1;
        warn!(restart_count = self.restart_count, "Restarting midi-client");
        self.spawn()
    }

    /// Reset backoff counter if the client has been stable for >60s.
    pub fn reset_backoff(&mut self) {
        if self.restart_count > 0 {
            if self.last_exit.map_or(false, |t| t.elapsed() > Duration::from_secs(60)) {
                self.restart_count = 0;
            }
        }
    }

    /// Send a graceful shutdown command via HTTP to the client's health server,
    /// then wait for the process to exit within the given timeout.
    /// Returns `true` if the process exited gracefully, `false` if force-killed.
    pub fn graceful_shutdown(&mut self, timeout: Duration) -> bool {
        // Send POST /shutdown to the health server (with connect timeout)
        let addr: std::net::SocketAddr = format!(
            "127.0.0.1:{}",
            midi_protocol::health::DEFAULT_HEALTH_PORT
        )
        .parse()
        .unwrap();

        if let Ok(mut stream) =
            std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        {
            use std::io::Write;
            let request = "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(request.as_bytes());
        }

        // Wait for the process to exit
        if let Some(ref mut child) = self.child {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        info!(exit_code = ?status.code(), "midi-client shut down gracefully");
                        self.child = None;
                        return true;
                    }
                    Ok(None) => {
                        if start.elapsed() >= timeout {
                            warn!("Client did not exit within timeout, killing");
                            let _ = child.kill();
                            let _ = child.wait();
                            self.child = None;
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => {
                        self.child = None;
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.graceful_shutdown(Duration::from_secs(5));
        }
    }
}

/// Check if Resolume Arena is running. Returns true if Arena.exe is found.
#[cfg(target_os = "windows")]
pub fn is_resolume_running() -> bool {
    Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Arena.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|out| {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("Arena.exe")
        })
        .unwrap_or(false)
}

/// Find the midi-client binary. Looks in the same directory as the tray exe first.
/// Checks both `midi-client` and `midinet-client` names (install script uses the latter).
pub fn find_client_binary() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let names = &["midi-client.exe", "midinet-client.exe"];
    #[cfg(not(target_os = "windows"))]
    let names = &["midi-client", "midinet-client"];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    // Fall back: check if it's on PATH
    #[cfg(target_os = "windows")]
    {
        for name in names {
            if let Ok(output) = Command::new("where").arg(name).output() {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout);
                    if let Some(first_line) = path.lines().next() {
                        let p = PathBuf::from(first_line.trim());
                        if p.exists() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        for name in names {
            if let Ok(output) = Command::new("which").arg(name).output() {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout);
                    let p = PathBuf::from(path.trim());
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}
