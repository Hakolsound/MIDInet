/// Programmatic tray icon generation.
///
/// Generates RGBA pixel data for colored status circles. No bundled image
/// files needed — icons are computed at startup and on state change.

use image::{Rgba, RgbaImage};
use tray_icon::Icon;

/// Icon size in pixels (fits all platforms: macOS 22px, Windows 16/32, Linux 22).
const ICON_SIZE: u32 = 32;

/// Status colors for the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconColor {
    /// Connected, healthy
    Green,
    /// Warnings (packet loss, task restart)
    Yellow,
    /// Disconnected or both hosts unreachable
    Red,
    /// Daemon not running / unreachable
    Gray,
}

/// Generate a tray icon with the given status color (full brightness).
pub fn generate_icon(color: IconColor) -> Icon {
    generate_icon_with_alpha(color, 255)
}

/// Generate a dimmed tray icon for the "off" phase of blinking.
pub fn generate_icon_dim(color: IconColor) -> Icon {
    generate_icon_with_alpha(color, 80)
}

/// Generate a tray icon with the given status color and base alpha.
fn generate_icon_with_alpha(color: IconColor, base_alpha: u8) -> Icon {
    let (r, g, b) = match color {
        IconColor::Green => (0x2E, 0xCC, 0x71),  // emerald green
        IconColor::Yellow => (0xF3, 0x9C, 0x12), // warm amber
        IconColor::Red => (0xE7, 0x4C, 0x3C),    // alert red
        IconColor::Gray => (0x95, 0xA5, 0xA6),   // neutral gray
    };

    let mut img = RgbaImage::new(ICON_SIZE, ICON_SIZE);
    let center = ICON_SIZE as f32 / 2.0;
    let radius = center - 2.0; // 2px padding for anti-aliasing

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= radius - 1.0 {
                // Fully inside the circle
                img.put_pixel(x, y, Rgba([r, g, b, base_alpha]));
            } else if dist <= radius + 1.0 {
                // Anti-aliased edge
                let edge_alpha = ((radius + 1.0 - dist) / 2.0 * base_alpha as f32) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, edge_alpha]));
            }
            // else: transparent (default)
        }
    }

    let rgba = img.into_raw();
    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).expect("failed to create tray icon")
}

/// Determine the icon color from a health snapshot.
pub fn color_for_snapshot(snapshot: &midi_protocol::health::ClientHealthSnapshot) -> IconColor {
    use midi_protocol::health::ConnectionState;

    match snapshot.connection_state {
        ConnectionState::Connected => {
            if snapshot.packet_loss_percent > 5.0 || !snapshot.watchdog.all_tasks_healthy {
                IconColor::Yellow
            } else {
                IconColor::Green
            }
        }
        ConnectionState::Discovering => IconColor::Yellow,
        ConnectionState::Reconnecting => IconColor::Red,
        ConnectionState::Disconnected => IconColor::Red,
    }
}
