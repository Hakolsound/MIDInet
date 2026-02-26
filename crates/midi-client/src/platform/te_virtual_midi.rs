/// teVirtualMIDI (Tobias Erichsen) backend for virtual MIDI devices.
/// Uses runtime DLL loading via LoadLibraryW — no compile-time .lib needed.
/// Tries teVirtualMIDI64.dll first (64-bit), then teVirtualMIDI.dll as fallback.
/// Requires teVirtualMIDI driver installed on the target system.
///
/// Gracefully returns Err if no DLL is found (caller handles fallback).

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

/// Shared feedback buffer for callback-mode ports.
/// The callback pushes data here; `receive()` pops from it.
#[cfg(target_os = "windows")]
type FeedbackBuffer = std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>;

/// teVirtualMIDI callback - invoked when other apps send MIDI to the virtual port.
/// Must be extern "system" (stdcall on Windows).
#[cfg(target_os = "windows")]
unsafe extern "system" fn midi_data_callback(
    _port: ffi::HANDLE,
    data: *const u8,
    length: ffi::DWORD,
    instance: *mut std::ffi::c_void,
) {
    if instance.is_null() || data.is_null() || length == 0 {
        return;
    }
    let buf = &*(instance as *const FeedbackBuffer);
    let slice = std::slice::from_raw_parts(data, length as usize);
    if let Ok(mut q) = buf.lock() {
        q.push_back(slice.to_vec());
        // Cap at 1024 entries to prevent unbounded growth
        while q.len() > 1024 {
            q.pop_front();
        }
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

        error!("teVirtualMIDI DLL not found");
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

    /// Try to create a port. Returns handle and whether MIDI Input was created.
    fn try_create_port(
        &self,
        wide_name: &[u16],
        flags: ffi::DWORD,
        use_callback: bool,
        label: &str,
        midi_in_before: u32,
        midi_out_before: u32,
        feedback_buf_ptr: *mut std::ffi::c_void,
    ) -> Option<(ffi::HANDLE, bool)> {
        let callback_ptr = if use_callback {
            midi_data_callback as *const std::ffi::c_void
        } else {
            std::ptr::null()
        };
        let instance_ptr = if use_callback { feedback_buf_ptr } else { std::ptr::null_mut() };

        let h = unsafe {
            (self.create_port_ex2)(
                wide_name.as_ptr(),
                callback_ptr,
                instance_ptr,
                65535,
                flags,
            )
        };

        if h.is_null() {
            let err = std::io::Error::last_os_error();
            warn!(
                flags = label,
                callback = use_callback,
                "Port creation failed: {}",
                err
            );
            return None;
        }

        // Wait for Windows to register the MIDI device
        std::thread::sleep(std::time::Duration::from_millis(300));

        let midi_in_now = unsafe { ffi::midiInGetNumDevs() };
        let midi_out_now = unsafe { ffi::midiOutGetNumDevs() };

        info!(
            flags = label,
            callback = use_callback,
            midi_in = format!("{} -> {}", midi_in_before, midi_in_now),
            midi_out = format!("{} -> {}", midi_out_before, midi_out_now),
            "Port created, checking device registration"
        );

        let input_ok = midi_in_now > midi_in_before;
        if input_ok {
            info!(flags = label, callback = use_callback, "MIDI Input device created!");
        } else if midi_out_now > midi_out_before {
            warn!(flags = label, callback = use_callback, "MIDI Output created but NOT Input");
        } else {
            warn!(flags = label, callback = use_callback, "Port handle valid but NO new MIDI devices registered");
        }

        Some((h, input_ok))
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

// ── TeVirtualMidiDevice ──

pub struct TeVirtualMidiDevice {
    name: String,
    #[cfg(target_os = "windows")]
    lib: Option<TeVirtualMidiLib>,
    #[cfg(target_os = "windows")]
    port: Mutex<Option<ffi::HANDLE>>,
    /// Feedback buffer for callback-mode ports. Pinned so the pointer stays stable.
    #[cfg(target_os = "windows")]
    feedback_buf: std::pin::Pin<Box<FeedbackBuffer>>,
    /// Whether the port was created with a callback (callback mode vs polling mode).
    #[cfg(target_os = "windows")]
    use_callback: std::sync::atomic::AtomicBool,
    /// When true, Drop skips port close — the kernel driver cleans up on process exit.
    #[cfg(target_os = "windows")]
    detached: bool,
    #[cfg(not(target_os = "windows"))]
    _phantom: (),
}

#[cfg(target_os = "windows")]
unsafe impl Send for TeVirtualMidiDevice {}
#[cfg(target_os = "windows")]
unsafe impl Sync for TeVirtualMidiDevice {}

impl TeVirtualMidiDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            #[cfg(target_os = "windows")]
            lib: TeVirtualMidiLib::load(),
            #[cfg(target_os = "windows")]
            port: Mutex::new(None),
            #[cfg(target_os = "windows")]
            feedback_buf: Box::pin(std::sync::Mutex::new(std::collections::VecDeque::new())),
            #[cfg(target_os = "windows")]
            use_callback: std::sync::atomic::AtomicBool::new(false),
            #[cfg(target_os = "windows")]
            detached: false,
            #[cfg(not(target_os = "windows"))]
            _phantom: (),
        }
    }
}

impl VirtualMidiDevice for TeVirtualMidiDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        #[cfg(target_os = "windows")]
        {
            let lib = match self.lib.as_ref() {
                Some(lib) => lib,
                None => {
                    return Err(anyhow::anyhow!(
                        "teVirtualMIDI DLL not found — driver not installed"
                    ));
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

            // Strategy: try WITH callback first (some SDK versions only register
            // MIDI Input when a callback is provided), then without callback.
            // For each, try the recommended flag combinations.
            struct Attempt {
                flags: ffi::DWORD,
                label: &'static str,
                callback: bool,
            }

            let attempts = [
                // WITH callback - most likely to register MIDI Input
                Attempt {
                    flags: ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    label: "PARSE_RX|INSTANTIATE_BOTH",
                    callback: true,
                },
                Attempt {
                    flags: ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_PARSE_TX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    label: "PARSE_RX|PARSE_TX|INSTANTIATE_BOTH",
                    callback: true,
                },
                Attempt {
                    flags: ffi::TE_VM_FLAGS_PARSE_RX,
                    label: "PARSE_RX (default=both)",
                    callback: true,
                },
                Attempt {
                    flags: 0,
                    label: "none",
                    callback: true,
                },
                // WITHOUT callback - fallback
                Attempt {
                    flags: ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    label: "PARSE_RX|INSTANTIATE_BOTH",
                    callback: false,
                },
                Attempt {
                    flags: ffi::TE_VM_FLAGS_PARSE_RX,
                    label: "PARSE_RX (default=both)",
                    callback: false,
                },
            ];

            let mut final_handle: ffi::HANDLE = std::ptr::null_mut();
            let mut final_label = "";
            let mut midi_input_ok = false;
            let mut final_uses_callback = false;

            // Get a raw pointer to the feedback buffer for the callback.
            // The buffer is Pin<Box<...>> so the pointer stays stable.
            let buf_ptr = &*self.feedback_buf as *const FeedbackBuffer as *mut std::ffi::c_void;

            for attempt in &attempts {
                if let Some((h, input_created)) = lib.try_create_port(
                    &wide_name,
                    attempt.flags,
                    attempt.callback,
                    attempt.label,
                    midi_in_before,
                    midi_out_before,
                    buf_ptr,
                ) {
                    if input_created {
                        final_handle = h;
                        final_label = attempt.label;
                        midi_input_ok = true;
                        final_uses_callback = attempt.callback;
                        break;
                    }

                    // Port created but no MIDI Input - close and try next
                    unsafe { (lib.close_port)(h) };
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }

            // If nothing created MIDI Input, use first working port as fallback
            if final_handle.is_null() {
                warn!("No attempt created MIDI Input - falling back to any working port");
                let h = unsafe {
                    (lib.create_port_ex2)(
                        wide_name.as_ptr(),
                        midi_data_callback as *const std::ffi::c_void,
                        buf_ptr,
                        65535,
                        ffi::TE_VM_FLAGS_PARSE_RX | ffi::TE_VM_FLAGS_INSTANTIATE_BOTH,
                    )
                };
                if !h.is_null() {
                    final_handle = h;
                    final_label = "fallback+callback";
                    final_uses_callback = true;
                }
            }

            if final_handle.is_null() {
                return Err(anyhow::anyhow!(
                    "teVirtualMIDI port creation failed — all flag combinations exhausted"
                ));
            }

            // Final diagnostics
            std::thread::sleep(std::time::Duration::from_millis(300));
            let midi_in_final = unsafe { ffi::midiInGetNumDevs() };
            let midi_out_final = unsafe { ffi::midiOutGetNumDevs() };

            if !midi_input_ok {
                // Close the port — it's useless without MIDI Input
                unsafe { (lib.close_port)(final_handle) };

                warn!(
                    dll = %lib.dll_name,
                    dll_path = %lib.dll_path,
                    flags = final_label,
                    midi_in = midi_in_final,
                    midi_out = midi_out_final,
                    "teVirtualMIDI: No MIDI Input device registered despite port creation succeeding. \
                     The kernel driver is loaded but not registering devices. \
                     Will attempt fallback to Windows MIDI Services if available."
                );

                return Err(anyhow::anyhow!(
                    "teVirtualMIDI port created but no MIDI Input registered — \
                     kernel driver not functioning correctly"
                ));
            }

            info!(
                flags = final_label,
                callback_mode = final_uses_callback,
                midi_in = midi_in_final,
                midi_out = midi_out_final,
                "Virtual MIDI port creation complete with MIDI Input"
            );

            self.use_callback.store(final_uses_callback, std::sync::atomic::Ordering::Relaxed);
            *self.port.lock().unwrap() = Some(final_handle);
        }

        #[cfg(not(target_os = "windows"))]
        {
            return Err(anyhow::anyhow!("teVirtualMIDI: not available on non-Windows"));
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
            // Callback mode: data was buffered by midi_data_callback
            if self.use_callback.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(mut q) = self.feedback_buf.lock() {
                    return Ok(q.pop_front());
                }
                return Ok(None);
            }

            // Polling mode: use virtualMIDIGetData directly
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
            if self.detached {
                info!(name = %self.name, "teVirtualMIDI device detached — skipping explicit close");
                return Ok(());
            }

            if let Some(ref lib) = self.lib {
                let mut guard = self.port.lock().unwrap();
                if let Some(handle) = guard.take() {
                    unsafe { (lib.close_port)(handle) };
                    info!(name = %self.name, "Closed teVirtualMIDI device");
                }
            }
        }
        Ok(())
    }

    fn silence_and_detach(&mut self) -> anyhow::Result<()> {
        self.send_all_off()?;

        #[cfg(target_os = "windows")]
        {
            self.detached = true;
            info!(
                name = %self.name,
                "teVirtualMIDI device silenced and detached — port stays alive until process exit"
            );
        }

        #[cfg(not(target_os = "windows"))]
        {
            self.close()?;
        }

        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}

#[cfg(target_os = "windows")]
impl Drop for TeVirtualMidiDevice {
    fn drop(&mut self) {
        if self.detached {
            // Detached mode: let the kernel driver clean up on process exit.
            // This prevents crashes in apps holding open handles to the port.
            return;
        }
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
