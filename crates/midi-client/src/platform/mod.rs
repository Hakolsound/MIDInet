#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod te_virtual_midi;
#[cfg(target_os = "windows")]
pub mod midi_services;
#[cfg(target_os = "windows")]
pub mod windows;
