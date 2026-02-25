/// Windows virtual MIDI device implementation.
/// Uses teVirtualMIDI (Tobias Erichsen) via runtime DLL loading.
/// Tries teVirtualMIDI64.dll first (64-bit), then teVirtualMIDI.dll as fallback.
/// Requires teVirtualMIDI driver installed on the target system.
///
/// Gracefully falls back to no-op if no DLL is found.

use std::sync::Mutex;

use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;
use tracing::{error, info, warn};

#[cfg(target_os = "windows")]
mod ffi {
    use std::ffi::c_void;

    pub type LPCWSTR = *const u16;
    pub type DWORD = u32;
    pub type BOOL = i32;
    pub type HANDLE = *mut c_void;
    pub type WORD = u16;
    pub type HMODULE = *mut c_void;
    pub type FARPROC = *mut c_void;

    // teVirtualMIDI flag constants
    pub const TE_VM_FLAGS_PARSE_RX: DWORD = 0x01;
    pub const TE_VM_FLAGS_PARSE_TX: DWORD = 0x02;
    pub const TE_VM_FLAGS_INSTANTIATE_BOTH: DWORD = 0x0C;

    // Function pointer types for teVirtualMIDI
    pub type FnCreatePortEx2 = unsafe extern "system" fn(
        port_name: LPCWSTR,
        callback: *const c_void,
        callback_instance: *mut c_void,
        max_sysex_length: DWORD,
        flags: DWORD,
    ) -> HANDLE;

    pub type FnClosePort = unsafe extern "system" fn(port: HANDLE);

    pub type FnSendData = unsafe extern "system" fn(
        port: HANDLE,
        data: *const u8,
        length: DWORD,
    ) -> BOOL;

    pub type FnGetData = unsafe extern "system" fn(
        port: HANDLE,
        data: *mut u8,
        length: *mut DWORD,
    ) -> BOOL;

    pub type FnGetVersion = unsafe extern "system" fn(
        major: *mut WORD,
        minor: *mut WORD,
        release: *mut WORD,
        build: *mut WORD,
    ) -> LPCWSTR;

    // kernel32 - always available on Windows
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(lpFileName: LPCWSTR) -> HMODULE;
        pub fn GetProcAddress(hModule: HMODULE, lpProcName: *const u8) -> FARPROC;
        pub fn FreeLibrary(hModule: HMODULE) -> BOOL;
        pub fn GetModuleFileNameW(hModule: HMODULE, lpFilename: *mut u16, nSize: DWORD) -> DWORD;
    }

    // winmm - always available on Windows
    #[link(name = "winmm")]
    extern "system" {
        pub fn midiInGetNumDevs() -> u32;
        pub fn midiOutGetNumDevs() -> u32;
    }
}

// ── Runtime-loaded teVirtualMIDI library ──

#[cfg(target_os = "windows")]
struct TeVirtualMidiLib {
    _dll: ffi::HMODULE,
    dll_name: String,
    dll_path: String,
    pub create_port_ex2: ffi::FnCreatePortEx2,
    pub close_port: ffi::FnClosePort,
    pub send_data: ffi::FnSendData,
    pub get_data: ffi::FnGetData,
    pub get_version: ffi::FnGetVersion,
    pub get_driver_version: ffi::FnGetVersion,
}

#[cfg(target_os = "windows")]
impl TeVirtualMidiLib {
    /// Try to load teVirtualMIDI. Tries 64-bit DLL first, then generic name.
    fn load() -> Option<Self> {
        let dll_names = [
            "teVirtualMIDI64.dll",
            "teVirtualMIDI.dll",
        ];

        for name in &dll_names {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let dll = unsafe { ffi::LoadLibraryW(wide.as_ptr()) };
            if dll.is_null() {
                info!(dll = %name, "DLL not found, trying next");
                continue;
            }

            // Get the actual file path of the loaded DLL
            let mut path_buf = [0u16; 512];
            let path_len = unsafe { ffi::GetModuleFileNameW(dll, path_buf.as_mut_ptr(), 512) };
            let dll_path = if path_len > 0 {
                String::from_utf16_lossy(&path_buf[..path_len as usize])
            } else {
                "(unknown path)".to_string()
            };

            info!(dll = %name, path = %dll_path, "Loaded teVirtualMIDI DLL");

            // Resolve all required function pointers
            macro_rules! resolve {
                ($fname:expr) => {{
                    let p = unsafe { ffi::GetProcAddress(dll, concat!($fname, "\0").as_ptr()) };
                    if p.is_null() {
                        warn!(dll = %name, func = $fname, "Function not exported");
                        unsafe { ffi::FreeLibrary(dll) };
                        continue;
                    }
                    p
                }};
            }

            let p_create = resolve!("virtualMIDICreatePortEx2");
            let p_close = resolve!("virtualMIDIClosePort");
            let p_send = resolve!("virtualMIDISendData");
            let p_get = resolve!("virtualMIDIGetData");
            let p_ver = resolve!("virtualMIDIGetVersion");
            let p_drv = resolve!("virtualMIDIGetDriverVersion");

            info!(dll = %name, "All function pointers resolved successfully");

            return Some(Self {
                _dll: dll,
                dll_name: name.to_string(),
                dll_path,
                create_port_ex2: unsafe { std::mem::transmute(p_create) },
                close_port: unsafe { std::mem::transmute(p_close) },
                send_data: unsafe { std::mem::transmute(p_send) },
                get_data: unsafe { std::mem::transmute(p_get) },
                get_version: unsafe { std::mem::transmute(p_ver) },
                get_driver_version: unsafe { std::mem::transmute(p_drv) },
            });
        }

        error!("teVirtualMIDI DLL not found. Install from https://www.tobias-erichsen.de/software/virtualmidi.html");
        None
    }

    fn log_versions(&self) {
        unsafe {
            let (mut maj, mut min, mut rel, mut bld) = (0u16, 0u16, 0u16, 0u16);

            let sdk_str = (self.get_version)(&mut maj, &mut min, &mut rel, &mut bld);
            let sdk_ver = wide_ptr_to_string(sdk_str);
            info!(
                dll = %self.dll_name,
                path = %self.dll_path,
                sdk_version = format!("{}.{}.{}.{}", maj, min, rel, bld),
                sdk_string = %sdk_ver,
                "teVirtualMIDI SDK version"
            );

            let drv_str = (self.get_driver_version)(&mut maj, &mut min, &mut rel, &mut bld);
            let drv_ver = wide_ptr_to_string(drv_str);
            info!(
                driver_version = format!("{}.{}.{}.{}", maj, min, rel, bld),
                driver_string = %drv_ver,
                "teVirtualMIDI driver version"
            );

            if maj == 0 && min == 0 && rel == 0 && bld == 0 {
                warn!("teVirtualMIDI driver version 0.0.0.0 - kernel driver may not be installed");
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for TeVirtualMidiLib {
    fn drop(&mut self) {
        unsafe { ffi::FreeLibrary(self._dll) };
    }
}

/// Convert a null-terminated wide string pointer to a Rust String.
#[cfg(target_os = "windows")]
unsafe fn wide_ptr_to_string(ptr: ffi::LPCWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

// ── WindowsVirtualDevice ──

pub struct WindowsVirtualDevice {
    name: String,
    #[cfg(target_os = "windows")]
    lib: Option<TeVirtualMidiLib>,
    #[cfg(target_os = "windows")]
    port: Mutex<Option<ffi::HANDLE>>,
    #[cfg(not(target_os = "windows"))]
    _phantom: (),
}

#[cfg(target_os = "windows")]
unsafe impl Send for WindowsVirtualDevice {}
#[cfg(target_os = "windows")]
unsafe impl Sync for WindowsVirtualDevice {}

impl WindowsVirtualDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            #[cfg(target_os = "windows")]
            lib: TeVirtualMidiLib::load(),
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
            let lib = match self.lib.as_ref() {
                Some(lib) => lib,
                None => {
                    warn!(name = %self.name, "teVirtualMIDI DLL not loaded - virtual MIDI device disabled");
                    return Ok(());
                }
            };

            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;

            // Log SDK and driver versions
            lib.log_versions();

            // Log MIDI device counts before creation
            let midi_in_before = unsafe { ffi::midiInGetNumDevs() };
            let midi_out_before = unsafe { ffi::midiOutGetNumDevs() };
            info!(
                midi_in = midi_in_before,
                midi_out = midi_out_before,
                "MIDI device counts BEFORE virtual port creation"
            );

            // Convert name to wide string (UTF-16 null-terminated)
            let wide_name: Vec<u16> = OsStr::new(&self.name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            // Try flag combinations, verify MIDI Input actually appears after each attempt
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
                (0, "none"),
            ];

            let mut final_handle: ffi::HANDLE = std::ptr::null_mut();
            let mut final_label = "";
            let mut midi_input_ok = false;

            for (flags, label) in flag_attempts {
                let h = unsafe {
                    (lib.create_port_ex2)(
                        wide_name.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        65535,
                        *flags,
                    )
                };

                if h.is_null() {
                    let err = std::io::Error::last_os_error();
                    warn!(flags = label, "Port creation failed: {}", err);
                    continue;
                }

                // Wait for Windows to register the MIDI device
                std::thread::sleep(std::time::Duration::from_millis(200));

                let midi_in_now = unsafe { ffi::midiInGetNumDevs() };
                let midi_out_now = unsafe { ffi::midiOutGetNumDevs() };

                info!(
                    flags = label,
                    flags_hex = format!("{:#x}", flags),
                    midi_in = format!("{} -> {}", midi_in_before, midi_in_now),
                    midi_out = format!("{} -> {}", midi_out_before, midi_out_now),
                    "Port created, checking device registration"
                );

                if midi_in_now > midi_in_before {
                    info!(flags = label, "MIDI Input device created successfully");
                    final_handle = h;
                    final_label = label;
                    midi_input_ok = true;
                    break;
                }

                // Also check if midi_out increased (at least the port IS registering something)
                if midi_out_now > midi_out_before {
                    info!(flags = label, "MIDI Output created but not Input - trying next flags");
                }

                warn!(flags = label, "Port handle valid but no new MIDI devices registered");
                unsafe { (lib.close_port)(h) };
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Fallback: use any port that at least returns a handle
            if final_handle.is_null() {
                warn!("No flags created MIDI Input - falling back to first working port");
                let h = unsafe {
                    (lib.create_port_ex2)(
                        wide_name.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        65535,
                        ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    )
                };
                if !h.is_null() {
                    final_handle = h;
                    final_label = "fallback (PARSE_RX|INSTANTIATE_BOTH)";
                }
            }

            if final_handle.is_null() {
                error!("All teVirtualMIDI port creation attempts failed");
                return Err(anyhow::anyhow!(
                    "teVirtualMIDI port creation failed. Install from https://www.tobias-erichsen.de/software/virtualmidi.html"
                ));
            }

            // Final diagnostics
            std::thread::sleep(std::time::Duration::from_millis(200));
            let midi_in_final = unsafe { ffi::midiInGetNumDevs() };
            let midi_out_final = unsafe { ffi::midiOutGetNumDevs() };

            if !midi_input_ok {
                error!(
                    dll = %lib.dll_name,
                    dll_path = %lib.dll_path,
                    flags = final_label,
                    midi_in = midi_in_final,
                    midi_out = midi_out_final,
                    "CRITICAL: Virtual MIDI port handle created but NO MIDI devices registered in Windows. \
                     The teVirtualMIDI kernel driver may not be properly installed. \
                     Try: (1) Install loopMIDI from https://www.tobias-erichsen.de/software/loopmidi.html \
                     (2) Or reinstall teVirtualMIDI SDK from https://www.tobias-erichsen.de/software/virtualmidi.html \
                     (3) Reboot after installation"
                );
            }

            info!(
                flags = final_label,
                midi_in = midi_in_final,
                midi_out = midi_out_final,
                input_created = midi_input_ok,
                "Virtual MIDI port creation complete"
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
            if let Some(ref lib) = self.lib {
                let guard = self.port.lock().unwrap();
                if let Some(handle) = *guard {
                    let ok = unsafe {
                        (lib.send_data)(handle, data.as_ptr(), data.len() as ffi::DWORD)
                    };
                    if ok == 0 {
                        let err = std::io::Error::last_os_error();
                        return Err(anyhow::anyhow!("virtualMIDISendData failed: {}", err));
                    }
                }
            }
        }
        let _ = data;
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        #[cfg(target_os = "windows")]
        {
            if let Some(ref lib) = self.lib {
                let guard = self.port.lock().unwrap();
                if let Some(handle) = *guard {
                    let mut buf = [0u8; 1024];
                    let mut len: ffi::DWORD = buf.len() as ffi::DWORD;
                    let ok = unsafe {
                        (lib.get_data)(handle, buf.as_mut_ptr(), &mut len)
                    };
                    if ok != 0 && len > 0 {
                        return Ok(Some(buf[..len as usize].to_vec()));
                    }
                }
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            if let Some(ref lib) = self.lib {
                let mut guard = self.port.lock().unwrap();
                if let Some(handle) = guard.take() {
                    unsafe { (lib.close_port)(handle) };
                    info!(name = %self.name, "Closed Windows virtual MIDI device");
                }
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
        // Close port before lib is dropped (lib drop calls FreeLibrary)
        if let Some(ref lib) = self.lib {
            if let Ok(mut guard) = self.port.lock() {
                if let Some(handle) = guard.take() {
                    unsafe { (lib.close_port)(handle) };
                }
            }
        }
    }
}
