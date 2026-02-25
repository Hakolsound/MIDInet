/// Windows virtual MIDI device implementation.
/// Uses teVirtualMIDI (Tobias Erichsen) via FFI for broad Windows compatibility (7-11).
/// Requires teVirtualMIDI driver installed on the target system.
///
/// Fallback: logs a warning and operates as a no-op if the DLL is not found.

use std::sync::Mutex;

use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;
use tracing::{error, info, warn};

// ── teVirtualMIDI FFI bindings ──
// These map to the teVirtualMIDI SDK C API.

#[cfg(target_os = "windows")]
mod ffi {
    use std::ffi::c_void;

    pub type LPCWSTR = *const u16;
    pub type DWORD = u32;
    pub type BOOL = i32;
    pub type HANDLE = *mut c_void;
    pub type WORD = u16;

    // teVirtualMIDI flag constants (SDK >= 1.3)
    pub const TE_VM_FLAGS_PARSE_RX: DWORD = 0x01;
    pub const TE_VM_FLAGS_PARSE_TX: DWORD = 0x02;
    pub const TE_VM_FLAGS_INSTANTIATE_RX_ONLY: DWORD = 0x04;
    pub const TE_VM_FLAGS_INSTANTIATE_TX_ONLY: DWORD = 0x08;
    pub const TE_VM_FLAGS_INSTANTIATE_BOTH: DWORD = 0x0C;

    #[link(name = "teVirtualMIDI")]
    extern "system" {
        pub fn virtualMIDICreatePortEx2(
            port_name: LPCWSTR,
            callback: *const c_void,
            callback_instance: *mut c_void,
            max_sysex_length: DWORD,
            flags: DWORD,
        ) -> HANDLE;

        pub fn virtualMIDIClosePort(port: HANDLE);

        pub fn virtualMIDISendData(
            port: HANDLE,
            data: *const u8,
            length: DWORD,
        ) -> BOOL;

        pub fn virtualMIDIGetData(
            port: HANDLE,
            data: *mut u8,
            length: *mut DWORD,
        ) -> BOOL;

        pub fn virtualMIDIGetVersion(
            major: *mut WORD,
            minor: *mut WORD,
            release: *mut WORD,
            build: *mut WORD,
        ) -> LPCWSTR;

        pub fn virtualMIDIGetDriverVersion(
            major: *mut WORD,
            minor: *mut WORD,
            release: *mut WORD,
            build: *mut WORD,
        ) -> LPCWSTR;
    }

    #[link(name = "winmm")]
    extern "system" {
        pub fn midiInGetNumDevs() -> u32;
        pub fn midiOutGetNumDevs() -> u32;
    }
}

pub struct WindowsVirtualDevice {
    name: String,
    #[cfg(target_os = "windows")]
    port: Mutex<Option<ffi::HANDLE>>,
    #[cfg(not(target_os = "windows"))]
    _phantom: (),
}

// teVirtualMIDI HANDLE is a raw pointer - safe to send across threads
// when access is synchronized via Mutex.
#[cfg(target_os = "windows")]
unsafe impl Send for WindowsVirtualDevice {}
#[cfg(target_os = "windows")]
unsafe impl Sync for WindowsVirtualDevice {}

impl WindowsVirtualDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            #[cfg(target_os = "windows")]
            port: Mutex::new(None),
            #[cfg(not(target_os = "windows"))]
            _phantom: (),
        }
    }
}

/// Read a null-terminated wide string pointer into a Rust String.
#[cfg(target_os = "windows")]
unsafe fn wide_to_string(ptr: ffi::LPCWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(ptr, len)).ok()
}

impl VirtualMidiDevice for WindowsVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        #[cfg(target_os = "windows")]
        {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;

            // ── Diagnostics: SDK and driver versions ──
            unsafe {
                let (mut maj, mut min, mut rel, mut bld) = (0u16, 0u16, 0u16, 0u16);

                let sdk_str = ffi::virtualMIDIGetVersion(&mut maj, &mut min, &mut rel, &mut bld);
                let sdk_ver = wide_to_string(sdk_str).unwrap_or_default();
                info!(
                    sdk_version = format!("{}.{}.{}.{}", maj, min, rel, bld),
                    sdk_string = %sdk_ver,
                    "teVirtualMIDI SDK"
                );

                let drv_str = ffi::virtualMIDIGetDriverVersion(&mut maj, &mut min, &mut rel, &mut bld);
                let drv_ver = wide_to_string(drv_str).unwrap_or_default();
                info!(
                    driver_version = format!("{}.{}.{}.{}", maj, min, rel, bld),
                    driver_string = %drv_ver,
                    "teVirtualMIDI driver"
                );

                if maj == 0 && min == 0 && rel == 0 && bld == 0 {
                    warn!("teVirtualMIDI driver version is 0.0.0.0 - driver may not be properly installed");
                }
            }

            // ── Diagnostics: MIDI devices before port creation ──
            let midi_in_before = unsafe { ffi::midiInGetNumDevs() };
            let midi_out_before = unsafe { ffi::midiOutGetNumDevs() };
            info!(
                midi_in = midi_in_before,
                midi_out = midi_out_before,
                "Windows MIDI device counts BEFORE virtual port creation"
            );

            // Convert name to wide string (UTF-16 null-terminated)
            let wide_name: Vec<u16> = OsStr::new(&self.name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            // Flag strategy:
            //   PARSE_RX (0x01) = Parse incoming MIDI data
            //   PARSE_TX (0x02) = Parse outgoing MIDI data
            //   INSTANTIATE_RX_ONLY (0x04) = Create MIDI Input port only
            //   INSTANTIATE_TX_ONLY (0x08) = Create MIDI Output port only
            //   INSTANTIATE_BOTH   (0x0C) = Create both Input and Output
            //   When no INSTANTIATE flags set: default should be BOTH (SDK >= 1.3)
            //
            // We try each combination, and after a successful port creation we check
            // whether MIDI Input was actually created (midiInGetNumDevs increased).
            // If not, we close the port and try the next flags.
            let flag_attempts: &[(ffi::DWORD, &str)] = &[
                (
                    ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    "PARSE_RX|INSTANTIATE_BOTH",
                ),
                (
                    ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_PARSE_TX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    "PARSE_RX|PARSE_TX|INSTANTIATE_BOTH",
                ),
                (
                    ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    "INSTANTIATE_BOTH",
                ),
                (
                    ffi::TE_VM_FLAGS_PARSE_RX,
                    "PARSE_RX (default=both)",
                ),
                (
                    ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_PARSE_TX,
                    "PARSE_RX|PARSE_TX",
                ),
                (0, "none"),
            ];

            let mut final_handle: ffi::HANDLE = std::ptr::null_mut();
            let mut final_flags_label: &str = "";
            let mut midi_input_created = false;

            for (flags, label) in flag_attempts {
                let h = unsafe {
                    ffi::virtualMIDICreatePortEx2(
                        wide_name.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        65535,
                        *flags,
                    )
                };

                if h.is_null() {
                    let err = std::io::Error::last_os_error();
                    warn!(
                        name = %self.name,
                        flags = label,
                        flags_hex = format!("{:#x}", flags),
                        "Port creation failed: {}",
                        err
                    );
                    continue;
                }

                // Port created - check if MIDI Input was registered
                // Give Windows a moment to register the device
                std::thread::sleep(std::time::Duration::from_millis(50));

                let midi_in_after = unsafe { ffi::midiInGetNumDevs() };
                let midi_out_after = unsafe { ffi::midiOutGetNumDevs() };

                info!(
                    name = %self.name,
                    flags = label,
                    flags_hex = format!("{:#x}", flags),
                    midi_in_before = midi_in_before,
                    midi_in_after = midi_in_after,
                    midi_out_before = midi_out_before,
                    midi_out_after = midi_out_after,
                    "Port created, checking MIDI device counts"
                );

                if midi_in_after > midi_in_before {
                    info!(
                        name = %self.name,
                        flags = label,
                        "MIDI Input device successfully created! Resolume will see this device."
                    );
                    final_handle = h;
                    final_flags_label = label;
                    midi_input_created = true;
                    break;
                }

                // MIDI Input not created with these flags - try next
                warn!(
                    name = %self.name,
                    flags = label,
                    "Port opened but MIDI Input NOT created (midiIn: {} -> {}). Trying next flags...",
                    midi_in_before,
                    midi_in_after
                );
                unsafe { ffi::virtualMIDIClosePort(h) };
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            // If no flags created MIDI Input, fall back to the first that at least creates a port
            if final_handle.is_null() {
                warn!("No flag combination created MIDI Input. Falling back to any working port...");
                for (flags, label) in flag_attempts {
                    let h = unsafe {
                        ffi::virtualMIDICreatePortEx2(
                            wide_name.as_ptr(),
                            std::ptr::null(),
                            std::ptr::null_mut(),
                            65535,
                            *flags,
                        )
                    };

                    if !h.is_null() {
                        final_handle = h;
                        final_flags_label = label;
                        break;
                    }
                }
            }

            if final_handle.is_null() {
                error!(
                    name = %self.name,
                    "All teVirtualMIDI flag combinations failed - is the driver installed?"
                );
                return Err(anyhow::anyhow!(
                    "teVirtualMIDI port creation failed. Install from https://www.tobias-erichsen.de/software/virtualmidi.html"
                ));
            }

            if !midi_input_created {
                error!(
                    name = %self.name,
                    flags = final_flags_label,
                    "WARNING: Virtual MIDI port created but MIDI Input device was NOT registered. \
                     Applications like Resolume will only see MIDI Output (cannot receive MIDI). \
                     This usually means the teVirtualMIDI driver needs to be updated. \
                     Download the latest from: https://www.tobias-erichsen.de/software/virtualmidi.html"
                );
            }

            // Final diagnostics: log all MIDI device names
            let midi_in_final = unsafe { ffi::midiInGetNumDevs() };
            let midi_out_final = unsafe { ffi::midiOutGetNumDevs() };
            info!(
                midi_in = midi_in_final,
                midi_out = midi_out_final,
                flags = final_flags_label,
                input_created = midi_input_created,
                "Final MIDI device counts after virtual port creation"
            );

            *self.port.lock().unwrap() = Some(final_handle);
        }

        #[cfg(not(target_os = "windows"))]
        {
            warn!(name = %self.name, "Windows virtual MIDI: compiled on non-Windows - no-op");
        }

        Ok(())
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            let guard = self.port.lock().unwrap();
            if let Some(handle) = *guard {
                let ok = unsafe {
                    ffi::virtualMIDISendData(handle, data.as_ptr(), data.len() as ffi::DWORD)
                };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(anyhow::anyhow!("virtualMIDISendData failed: {}", err));
                }
            }
        }
        let _ = data;
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        #[cfg(target_os = "windows")]
        {
            let guard = self.port.lock().unwrap();
            if let Some(handle) = *guard {
                let mut buf = [0u8; 1024];
                let mut len: ffi::DWORD = buf.len() as ffi::DWORD;
                let ok = unsafe {
                    ffi::virtualMIDIGetData(handle, buf.as_mut_ptr(), &mut len)
                };
                if ok != 0 && len > 0 {
                    return Ok(Some(buf[..len as usize].to_vec()));
                }
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            let mut guard = self.port.lock().unwrap();
            if let Some(handle) = guard.take() {
                unsafe { ffi::virtualMIDIClosePort(handle) };
                info!(name = %self.name, "Closed Windows virtual MIDI device");
            }
        }
        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsVirtualDevice {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.port.lock() {
            if let Some(handle) = guard.take() {
                unsafe { ffi::virtualMIDIClosePort(handle) };
            }
        }
    }
}
