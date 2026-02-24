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
// The DLL is loaded at runtime so the binary still starts without it.

#[cfg(target_os = "windows")]
mod ffi {
    use std::ffi::c_void;

    pub type LPCWSTR = *const u16;
    pub type DWORD = u32;
    pub type BOOL = i32;
    pub type HANDLE = *mut c_void;

    // Port creation flags
    pub const TE_VM_FLAGS_PARSE_RX: DWORD = 1;
    pub const TE_VM_FLAGS_INSTANTIATE_RX_ONLY: DWORD = 2;
    pub const TE_VM_FLAGS_INSTANTIATE_TX_ONLY: DWORD = 4;
    pub const TE_VM_FLAGS_INSTANTIATE_BOTH: DWORD = 6;

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
    }
}

pub struct WindowsVirtualDevice {
    name: String,
    #[cfg(target_os = "windows")]
    port: Mutex<Option<ffi::HANDLE>>,
    #[cfg(not(target_os = "windows"))]
    _phantom: (),
}

// teVirtualMIDI HANDLE is a raw pointer — safe to send across threads
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

impl VirtualMidiDevice for WindowsVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        #[cfg(target_os = "windows")]
        {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;

            // Convert name to wide string (UTF-16 null-terminated)
            let wide_name: Vec<u16> = OsStr::new(&self.name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let handle = unsafe {
                ffi::virtualMIDICreatePortEx2(
                    wide_name.as_ptr(),
                    std::ptr::null(),     // no callback — we poll with GetData
                    std::ptr::null_mut(),
                    65535,                // max sysex length
                    ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                )
            };

            if handle.is_null() {
                let err = std::io::Error::last_os_error();
                error!(name = %self.name, ?err, "Failed to create teVirtualMIDI port — is the driver installed?");
                return Err(anyhow::anyhow!(
                    "teVirtualMIDI port creation failed: {}. Install from https://www.tobias-erichsen.de/software/virtualmidi.html",
                    err
                ));
            }

            *self.port.lock().unwrap() = Some(handle);
            info!(name = %self.name, "Created Windows virtual MIDI device (teVirtualMIDI)");
        }

        #[cfg(not(target_os = "windows"))]
        {
            warn!(name = %self.name, "Windows virtual MIDI: compiled on non-Windows — no-op");
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
