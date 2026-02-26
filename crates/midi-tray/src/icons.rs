/// Programmatic tray icon generation with caching.
///
/// Generates RGBA pixel data for colored status circles. Icons are pre-computed
/// at startup and stored in an `IconCache` — no runtime pixel computation needed.

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

/// Pre-computed icon cache — all variants generated once at startup.
pub struct IconCache {
    green: CachedIcon,
    green_dim: CachedIcon,
    yellow: CachedIcon,
    red: CachedIcon,
    gray: CachedIcon,
}

struct CachedIcon {
    rgba: Vec<u8>,
}

impl CachedIcon {
    fn to_icon(&self) -> Icon {
        Icon::from_rgba(self.rgba.clone(), ICON_SIZE, ICON_SIZE)
            .expect("cached icon data is always valid")
    }
}

impl IconCache {
    /// Pre-generate all icon variants. Call once at startup.
    pub fn new() -> Self {
        Self {
            green: CachedIcon { rgba: render_circle(IconColor::Green, 255) },
            green_dim: CachedIcon { rgba: render_circle(IconColor::Green, 80) },
            yellow: CachedIcon { rgba: render_circle(IconColor::Yellow, 255) },
            red: CachedIcon { rgba: render_circle(IconColor::Red, 255) },
            gray: CachedIcon { rgba: render_circle(IconColor::Gray, 255) },
        }
    }

    /// Get an icon for the given color and dim state.
    pub fn get(&self, color: IconColor, dim: bool) -> Icon {
        match (color, dim) {
            (IconColor::Green, true) => self.green_dim.to_icon(),
            (IconColor::Green, false) => self.green.to_icon(),
            (IconColor::Yellow, _) => self.yellow.to_icon(),
            (IconColor::Red, _) => self.red.to_icon(),
            (IconColor::Gray, _) => self.gray.to_icon(),
        }
    }
}

/// Render a colored circle to RGBA pixel data.
fn render_circle(color: IconColor, base_alpha: u8) -> Vec<u8> {
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
                img.put_pixel(x, y, Rgba([r, g, b, base_alpha]));
            } else if dist <= radius + 1.0 {
                let edge_alpha = ((radius + 1.0 - dist) / 2.0 * base_alpha as f32) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, edge_alpha]));
            }
        }
    }

    img.into_raw()
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
