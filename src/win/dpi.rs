//! Thread-scoped DPI awareness helper for Windows plugin contexts.
//!
//! The original code called `SetProcessDpiAwarenessContext` from a plugin DLL,
//! which is a documented Microsoft anti-pattern: it changes the host process's
//! DPI awareness, stripping the bitmap-stretching compatibility shim from
//! DPI-unaware hosts (Audacity, Ableton Live ≤11, FL Studio, Bitwig, Cubase
//! ≤10.5, Pro Tools). The visible symptom is the host window collapsing to
//! native pixels on high-scale displays.
//!
//! This module replaces that call with the JUCE/iPlug2 pattern: dynamically
//! resolve `SetThreadDpiAwarenessContext` from user32.dll and apply it as an
//! RAII guard around `CreateWindowExW`. The host process's awareness stays
//! untouched; only the child window we create gets Per-Monitor-Aware-V2.
//!
//! No-op on pre-Win10 1607 where the thread-scoped API doesn't exist.

use std::ffi::CString;
use std::ptr::null_mut;
use std::sync::OnceLock;

use winapi::shared::minwindef::{FARPROC, HMODULE};
use winapi::shared::windef::DPI_AWARENESS_CONTEXT;
use winapi::um::libloaderapi::{GetModuleHandleA, GetProcAddress};

pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: DPI_AWARENESS_CONTEXT =
    -4isize as DPI_AWARENESS_CONTEXT;

type SetThreadDpiAwarenessContextFn =
    unsafe extern "system" fn(DPI_AWARENESS_CONTEXT) -> DPI_AWARENESS_CONTEXT;

struct DpiApi {
    set_thread: Option<SetThreadDpiAwarenessContextFn>,
}

fn api() -> &'static DpiApi {
    static API: OnceLock<DpiApi> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let module_name = CString::new("user32.dll").unwrap();
        let user32 = GetModuleHandleA(module_name.as_ptr());
        let resolve = |name: &str| -> FARPROC {
            if user32.is_null() {
                null_mut()
            } else {
                let cname = CString::new(name).unwrap();
                GetProcAddress(user32 as HMODULE, cname.as_ptr())
            }
        };
        DpiApi {
            set_thread: std::mem::transmute::<FARPROC, Option<SetThreadDpiAwarenessContextFn>>(
                resolve("SetThreadDpiAwarenessContext"),
            ),
        }
    })
}

/// RAII scope guard: sets the current thread's DPI awareness context on
/// construction, restores the previous one on drop. No-op on pre-1607
/// Windows where `SetThreadDpiAwarenessContext` is not available.
pub struct ThreadDpiAwarenessScope {
    previous: Option<DPI_AWARENESS_CONTEXT>,
}

impl ThreadDpiAwarenessScope {
    pub fn enter(target: DPI_AWARENESS_CONTEXT) -> Self {
        let previous = api().set_thread.and_then(|f| {
            let prev = unsafe { f(target) };
            if prev.is_null() {
                None
            } else {
                Some(prev)
            }
        });
        Self { previous }
    }
}

impl Drop for ThreadDpiAwarenessScope {
    fn drop(&mut self) {
        if let (Some(f), Some(prev)) = (api().set_thread, self.previous) {
            unsafe { f(prev) };
        }
    }
}
