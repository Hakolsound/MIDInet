/// WebSocket client that connects to the daemon's local health endpoint.
///
/// Runs on a background thread (sync tungstenite, not async) because the
/// main thread is occupied by the native GUI event loop. All TCP operations
/// use explicit timeouts to prevent thread hangs.

use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use tungstenite::Message;
use tracing::{debug, info, warn};

use midi_protocol::health::{ClientHealthSnapshot, DEFAULT_HEALTH_PORT};

/// Messages sent from the WS thread to the main/GUI thread.
pub enum WsEvent {
    /// New health snapshot from the daemon
    Snapshot(ClientHealthSnapshot),
    /// Connection lost — daemon unreachable
    Disconnected,
    /// Successfully (re)connected
    Connected,
}

/// Spawn the WebSocket client thread. Returns a receiver for events.
pub fn spawn_ws_thread() -> mpsc::Receiver<WsEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("midinet-ws".into())
        .spawn(move || ws_loop(tx))
        .expect("failed to spawn WS thread");
    rx
}

fn ws_loop(tx: mpsc::Sender<WsEvent>) {
    let url = format!("ws://127.0.0.1:{}/ws", DEFAULT_HEALTH_PORT);
    let addr: SocketAddr = format!("127.0.0.1:{}", DEFAULT_HEALTH_PORT)
        .parse()
        .unwrap();
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(8);

    loop {
        info!(url = %url, "Connecting to daemon health endpoint");

        // Connect with explicit timeout (prevents indefinite block if port is in limbo)
        let tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(stream) => stream,
            Err(e) => {
                debug!("Cannot connect to daemon: {}", e);
                let _ = tx.send(WsEvent::Disconnected);
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        // Set read timeout so socket.read() doesn't block forever if daemon freezes
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));

        // Upgrade TCP to WebSocket
        match tungstenite::client::client(&url, tcp) {
            Ok((mut socket, _response)) => {
                let _ = tx.send(WsEvent::Connected);
                backoff = Duration::from_secs(1); // reset on success

                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            match serde_json::from_str::<ClientHealthSnapshot>(&text) {
                                Ok(snapshot) => {
                                    if tx.send(WsEvent::Snapshot(snapshot)).is_err() {
                                        return; // main thread dropped the receiver
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to parse health snapshot: {}", e);
                                }
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            let _ = socket.send(Message::Pong(data));
                        }
                        Ok(Message::Close(_)) => {
                            info!("Daemon closed WebSocket connection");
                            break;
                        }
                        Ok(_) => {} // Binary, Pong, Frame
                        Err(e) => {
                            warn!("WebSocket read error: {}", e);
                            break;
                        }
                    }
                }

                let _ = tx.send(WsEvent::Disconnected);
            }
            Err(e) => {
                debug!("WebSocket handshake failed: {}", e);
                let _ = tx.send(WsEvent::Disconnected);
            }
        }

        // Backoff before reconnecting
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Send a command to the daemon (e.g. focus claim/release).
pub fn send_command(cmd: &midi_protocol::health::TrayCommand) {
    let endpoint = match cmd {
        midi_protocol::health::TrayCommand::ClaimFocus => "/focus/claim",
        midi_protocol::health::TrayCommand::ReleaseFocus => "/focus/release",
        midi_protocol::health::TrayCommand::Shutdown => "/shutdown",
    };

    // Simple synchronous HTTP POST (fire-and-forget from the GUI thread)
    std::thread::spawn({
        let endpoint = endpoint.to_string();
        move || {
            let addr: SocketAddr = format!("127.0.0.1:{}", DEFAULT_HEALTH_PORT)
                .parse()
                .unwrap();

            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                use std::io::Write;
                let request = format!(
                    "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    endpoint, DEFAULT_HEALTH_PORT
                );
                let _ = stream.write_all(request.as_bytes());
            }
        }
    });
}
