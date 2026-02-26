// Re-export platform backends from midi-device crate.
// Platform implementations live in midi-device so they can be shared
// with midi-bridge. This module re-exports them for backward compatibility.
#![allow(unused_imports)]

#[cfg(target_os = "linux")]
pub use midi_device::platform::linux;

#[cfg(target_os = "macos")]
pub use midi_device::platform::macos;

#[cfg(target_os = "windows")]
pub use midi_device::platform::te_virtual_midi;
#[cfg(target_os = "windows")]
pub use midi_device::platform::midi_services;
#[cfg(target_os = "windows")]
pub use midi_device::platform::windows;
