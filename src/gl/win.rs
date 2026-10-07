use std::ffi::{c_void, CString, OsStr};
use std::os::windows::ffi::OsStrExt;

use raw_window_handle::RawWindowHandle;

use winapi::shared::minwindef::{HINSTANCE, HMODULE};
use winapi::shared::ntdef::WCHAR;
use winapi::shared::windef::{HDC, HGLRC, HWND};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryA};
use winapi::um::wingdi::{
    wglCreateContext, wglDeleteContext, wglGetProcAddress, wglMakeCurrent, ChoosePixelFormat,
    DescribePixelFormat, SetPixelFormat, SwapBuffers, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW,
    PFD_GENERIC_ACCELERATED, PFD_GENERIC_FORMAT, PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA,
    PIXELFORMATDESCRIPTOR,
};
use winapi::um::winnt::IMAGE_DOS_HEADER;
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetDC, RegisterClassW, ReleaseDC,
    UnregisterClassW, CS_OWNDC, CW_USEDEFAULT, WNDCLASSW,
};

use super::{GlConfig, GlError, Profile};

// See https://www.khronos.org/registry/OpenGL/extensions/ARB/WGL_ARB_create_context.txt

type WglCreateContextAttribsARB = extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC;

const WGL_CONTEXT_MAJOR_VERSION_ARB: i32 = 0x2091;
const WGL_CONTEXT_MINOR_VERSION_ARB: i32 = 0x2092;
const WGL_CONTEXT_PROFILE_MASK_ARB: i32 = 0x9126;

const WGL_CONTEXT_CORE_PROFILE_BIT_ARB: i32 = 0x00000001;
const WGL_CONTEXT_COMPATIBILITY_PROFILE_BIT_ARB: i32 = 0x00000002;

// See https://www.khronos.org/registry/OpenGL/extensions/ARB/WGL_ARB_pixel_format.txt

type WglChoosePixelFormatARB =
    extern "system" fn(HDC, *const i32, *const f32, u32, *mut i32, *mut u32) -> i32;

const WGL_DRAW_TO_WINDOW_ARB: i32 = 0x2001;
const WGL_ACCELERATION_ARB: i32 = 0x2003;
const WGL_SUPPORT_OPENGL_ARB: i32 = 0x2010;
const WGL_DOUBLE_BUFFER_ARB: i32 = 0x2011;
const WGL_PIXEL_TYPE_ARB: i32 = 0x2013;
const WGL_RED_BITS_ARB: i32 = 0x2015;
const WGL_GREEN_BITS_ARB: i32 = 0x2017;
const WGL_BLUE_BITS_ARB: i32 = 0x2019;
const WGL_ALPHA_BITS_ARB: i32 = 0x201B;
const WGL_DEPTH_BITS_ARB: i32 = 0x2022;
const WGL_STENCIL_BITS_ARB: i32 = 0x2023;

const WGL_FULL_ACCELERATION_ARB: i32 = 0x2027;
const WGL_TYPE_RGBA_ARB: i32 = 0x202B;

// See https://www.khronos.org/registry/OpenGL/extensions/ARB/ARB_multisample.txt

const WGL_SAMPLE_BUFFERS_ARB: i32 = 0x2041;
const WGL_SAMPLES_ARB: i32 = 0x2042;

// See https://www.khronos.org/registry/OpenGL/extensions/ARB/ARB_framebuffer_sRGB.txt

const WGL_FRAMEBUFFER_SRGB_CAPABLE_ARB: i32 = 0x20A9;
const GL_FRAMEBUFFER_SRGB: u32 = 0x8DB9;

// See https://www.khronos.org/registry/OpenGL/extensions/EXT/WGL_EXT_swap_control.txt

type WglSwapIntervalEXT = extern "system" fn(i32) -> i32;

pub type CreationFailedError = ();
pub struct GlContext {
    hwnd: HWND,
    hdc: HDC,
    hglrc: HGLRC,
    gl_library: HMODULE,
}

extern "C" {
    static __ImageBase: IMAGE_DOS_HEADER;
}

impl GlContext {
    pub unsafe fn create(parent: &RawWindowHandle, config: GlConfig) -> Result<GlContext, GlError> {
        let handle = if let RawWindowHandle::Win32(handle) = parent {
            handle
        } else {
            return Err(GlError::InvalidWindowHandle);
        };

        if handle.hwnd.is_null() {
            return Err(GlError::InvalidWindowHandle);
        }

        // Create temporary window and context to load function pointers

        let class_name_str = format!("raw-gl-context-window-{}", uuid::Uuid::new_v4().to_simple());
        let mut class_name: Vec<WCHAR> = OsStr::new(&class_name_str).encode_wide().collect();
        class_name.push(0);

        let hinstance = &__ImageBase as *const IMAGE_DOS_HEADER as HINSTANCE;

        let wnd_class = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: hinstance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };

        let class = RegisterClassW(&wnd_class);
        if class == 0 {
            return Err(GlError::CreationFailed(()));
        }

        let hwnd_tmp = CreateWindowExW(
            0,
            class as *const WCHAR,
            [0].as_ptr(),
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null_mut(),
        );

        if hwnd_tmp.is_null() {
            return Err(GlError::CreationFailed(()));
        }

        let hdc_tmp = GetDC(hwnd_tmp);

        let pfd_tmp = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            cDepthBits: 24,
            cStencilBits: 8,
            iLayerType: PFD_MAIN_PLANE,
            ..std::mem::zeroed()
        };

        SetPixelFormat(hdc_tmp, ChoosePixelFormat(hdc_tmp, &pfd_tmp), &pfd_tmp);

        let hglrc_tmp = wglCreateContext(hdc_tmp);
        if hglrc_tmp.is_null() {
            ReleaseDC(hwnd_tmp, hdc_tmp);
            UnregisterClassW(class as *const WCHAR, hinstance);
            DestroyWindow(hwnd_tmp);
            return Err(GlError::CreationFailed(()));
        }

        wglMakeCurrent(hdc_tmp, hglrc_tmp);

        #[allow(non_snake_case)]
        let wglCreateContextAttribsARB: Option<WglCreateContextAttribsARB> = {
            let symbol = CString::new("wglCreateContextAttribsARB").unwrap();
            let addr = wglGetProcAddress(symbol.as_ptr());
            if !addr.is_null() {
                #[allow(clippy::missing_transmute_annotations)]
                Some(std::mem::transmute(addr))
            } else {
                None
            }
        };

        #[allow(non_snake_case)]
        let wglChoosePixelFormatARB: Option<WglChoosePixelFormatARB> = {
            let symbol = CString::new("wglChoosePixelFormatARB").unwrap();
            let addr = wglGetProcAddress(symbol.as_ptr());
            if !addr.is_null() {
                #[allow(clippy::missing_transmute_annotations)]
                Some(std::mem::transmute(addr))
            } else {
                None
            }
        };

        #[allow(non_snake_case)]
        let wglSwapIntervalEXT: Option<WglSwapIntervalEXT> = {
            let symbol = CString::new("wglSwapIntervalEXT").unwrap();
            let addr = wglGetProcAddress(symbol.as_ptr());
            if !addr.is_null() {
                #[allow(clippy::missing_transmute_annotations)]
                Some(std::mem::transmute(addr))
            } else {
                None
            }
        };

        wglMakeCurrent(hdc_tmp, std::ptr::null_mut());
        wglDeleteContext(hglrc_tmp);
        ReleaseDC(hwnd_tmp, hdc_tmp);
        UnregisterClassW(class as *const WCHAR, hinstance);
        DestroyWindow(hwnd_tmp);

        // Create actual context

        let hwnd = handle.hwnd as HWND;

        let hdc = GetDC(hwnd);

        let Some((pixel_format, source)) =
            choose_pixel_format(hdc, &config, wglChoosePixelFormatARB)
        else {
            log::error!("baseview: no usable OpenGL pixel format found; cannot create GL context");
            ReleaseDC(hwnd, hdc);
            return Err(GlError::CreationFailed(()));
        };

        let mut pfd: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
        DescribePixelFormat(
            hdc,
            pixel_format,
            std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u32,
            &mut pfd,
        );
        // A window's pixel format can only be set once, so there is no fallback past this point.
        if SetPixelFormat(hdc, pixel_format, &pfd) == 0 {
            log::error!(
                "baseview: SetPixelFormat({pixel_format}) failed (GetLastError = {})",
                GetLastError()
            );
            ReleaseDC(hwnd, hdc);
            return Err(GlError::CreationFailed(()));
        }
        log::info!("baseview: using OpenGL pixel format {pixel_format} ({source})");

        let profile_mask = match config.profile {
            Profile::Core => WGL_CONTEXT_CORE_PROFILE_BIT_ARB,
            Profile::Compatibility => WGL_CONTEXT_COMPATIBILITY_PROFILE_BIT_ARB,
        };

        #[rustfmt::skip]
        let ctx_attribs = [
            WGL_CONTEXT_MAJOR_VERSION_ARB, config.version.0 as i32,
            WGL_CONTEXT_MINOR_VERSION_ARB, config.version.1 as i32,
            WGL_CONTEXT_PROFILE_MASK_ARB, profile_mask,
            0
        ];

        #[allow(non_snake_case)]
        let Some(wglCreateContextAttribsARB) = wglCreateContextAttribsARB
        else {
            log::error!(
                "baseview: wglCreateContextAttribsARB is unavailable; cannot create GL context"
            );
            ReleaseDC(hwnd, hdc);
            return Err(GlError::CreationFailed(()));
        };
        let hglrc = wglCreateContextAttribsARB(hdc, std::ptr::null_mut(), ctx_attribs.as_ptr());
        if hglrc.is_null() {
            log::error!(
                "baseview: wglCreateContextAttribsARB failed for OpenGL {}.{} (GetLastError = {})",
                config.version.0,
                config.version.1,
                GetLastError()
            );
            ReleaseDC(hwnd, hdc);
            return Err(GlError::CreationFailed(()));
        }

        let gl_library_name = CString::new("opengl32.dll").unwrap();
        let gl_library = LoadLibraryA(gl_library_name.as_ptr());

        wglMakeCurrent(hdc, hglrc);
        if let Some(wgl_swap_interval_ext) = wglSwapIntervalEXT {
            wgl_swap_interval_ext(config.vsync as i32);
        } else {
            log::warn!("baseview: wglSwapIntervalEXT is unavailable; vsync left at driver default");
        }
        // The chosen format may be sRGB-capable (it is the first candidate). Such a framebuffer
        // only encodes on write while GL_FRAMEBUFFER_SRGB is enabled; egui_glow composites in
        // gamma space and only disables it when its extension-string detection succeeds, which
        // is unreliable on core profiles on some Windows drivers. Disable it here (core since
        // GL 3.0) so output is never double gamma-corrected, whichever format was chosen.
        let gl_disable_name = CString::new("glDisable").unwrap();
        let gl_disable = GetProcAddress(gl_library, gl_disable_name.as_ptr());
        if !gl_disable.is_null() {
            let gl_disable: extern "system" fn(u32) = std::mem::transmute(gl_disable);
            gl_disable(GL_FRAMEBUFFER_SRGB);
        }
        wglMakeCurrent(hdc, std::ptr::null_mut());

        Ok(GlContext { hwnd, hdc, hglrc, gl_library })
    }

    pub unsafe fn make_current(&self) {
        wglMakeCurrent(self.hdc, self.hglrc);
    }

    pub unsafe fn make_not_current(&self) {
        wglMakeCurrent(self.hdc, std::ptr::null_mut());
    }

    pub fn get_proc_address(&self, symbol: &str) -> *const c_void {
        let symbol = CString::new(symbol).unwrap();
        let addr = unsafe { wglGetProcAddress(symbol.as_ptr()) as *const c_void };
        if !addr.is_null() {
            addr
        } else {
            unsafe { GetProcAddress(self.gl_library, symbol.as_ptr()) as *const c_void }
        }
    }

    pub fn swap_buffers(&self) {
        unsafe {
            SwapBuffers(self.hdc);
        }
    }
}

impl Drop for GlContext {
    fn drop(&mut self) {
        unsafe {
            wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut());
            wglDeleteContext(self.hglrc);
            ReleaseDC(self.hwnd, self.hdc);
            FreeLibrary(self.gl_library);
        }
    }
}

/// Picks a pixel format for `hdc`, trying candidates in order of how likely they are to work and
/// validating each one. Every rejected candidate is logged before moving on to the next.
///
/// sRGB capability is not used to control gamma (see the `glDisable(GL_FRAMEBUFFER_SRGB)` in
/// [`GlContext::create`]), so `config.srgb` is deliberately ignored here: an sRGB-capable format
/// is simply the most widely available one. Forcing a non-sRGB-capable format first (3e5975e)
/// crashed the plug-in host with an access violation on an AMD Radeon Vega 8 machine.
#[allow(non_snake_case)]
unsafe fn choose_pixel_format(
    hdc: HDC, config: &GlConfig, wglChoosePixelFormatARB: Option<WglChoosePixelFormatARB>,
) -> Option<(i32, &'static str)> {
    if let Some(wglChoosePixelFormatARB) = wglChoosePixelFormatARB {
        #[rustfmt::skip]
        let base_attribs = [
            WGL_DRAW_TO_WINDOW_ARB, 1,
            WGL_ACCELERATION_ARB, WGL_FULL_ACCELERATION_ARB,
            WGL_SUPPORT_OPENGL_ARB, 1,
            WGL_DOUBLE_BUFFER_ARB, config.double_buffer as i32,
            WGL_PIXEL_TYPE_ARB, WGL_TYPE_RGBA_ARB,
            WGL_RED_BITS_ARB, config.red_bits as i32,
            WGL_GREEN_BITS_ARB, config.green_bits as i32,
            WGL_BLUE_BITS_ARB, config.blue_bits as i32,
            WGL_ALPHA_BITS_ARB, config.alpha_bits as i32,
            WGL_DEPTH_BITS_ARB, config.depth_bits as i32,
            WGL_STENCIL_BITS_ARB, config.stencil_bits as i32,
            WGL_SAMPLE_BUFFERS_ARB, config.samples.is_some() as i32,
            WGL_SAMPLES_ARB, config.samples.unwrap_or(0) as i32,
        ];

        // `None` omits WGL_FRAMEBUFFER_SRGB_CAPABLE_ARB entirely, for drivers that don't support
        // the sRGB extension (naming an unsupported attribute makes the whole request fail).
        let candidates: [(Option<i32>, &'static str); 3] =
            [(Some(1), "sRGB-capable"), (Some(0), "non-sRGB-capable"), (None, "no sRGB attribute")];
        for (srgb_capable, label) in candidates {
            let mut attribs = base_attribs.to_vec();
            if let Some(value) = srgb_capable {
                attribs.extend_from_slice(&[WGL_FRAMEBUFFER_SRGB_CAPABLE_ARB, value]);
            }
            attribs.push(0);

            let mut pixel_format = 0;
            let mut num_formats = 0;
            let ok = wglChoosePixelFormatARB(
                hdc,
                attribs.as_ptr(),
                std::ptr::null(),
                1,
                &mut pixel_format,
                &mut num_formats,
            );
            if ok == 0 {
                log::error!(
                    "baseview: wglChoosePixelFormatARB failed for {label} format \
                     (GetLastError = {}); trying next fallback",
                    GetLastError()
                );
                continue;
            }
            if num_formats == 0 {
                log::error!("baseview: no {label} pixel format available; trying next fallback");
                continue;
            }
            match validate_pixel_format(hdc, pixel_format, config) {
                Ok(()) => return Some((pixel_format, label)),
                Err(reason) => log::error!(
                    "baseview: rejected {label} pixel format {pixel_format}: {reason}; \
                     trying next fallback"
                ),
            }
        }
    } else {
        log::error!("baseview: wglChoosePixelFormatARB is unavailable; trying ChoosePixelFormat");
    }

    // Last resort: the pre-WGL-extension API, which returns the closest match rather than
    // requiring an exact one.
    let pfd = PIXELFORMATDESCRIPTOR {
        nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
        nVersion: 1,
        dwFlags: PFD_DRAW_TO_WINDOW
            | PFD_SUPPORT_OPENGL
            | if config.double_buffer { PFD_DOUBLEBUFFER } else { 0 },
        iPixelType: PFD_TYPE_RGBA,
        cColorBits: config
            .red_bits
            .saturating_add(config.green_bits)
            .saturating_add(config.blue_bits),
        cAlphaBits: config.alpha_bits,
        cDepthBits: config.depth_bits,
        cStencilBits: config.stencil_bits,
        iLayerType: PFD_MAIN_PLANE,
        ..std::mem::zeroed()
    };
    let pixel_format = ChoosePixelFormat(hdc, &pfd);
    if pixel_format == 0 {
        log::error!("baseview: ChoosePixelFormat failed (GetLastError = {})", GetLastError());
        return None;
    }
    match validate_pixel_format(hdc, pixel_format, config) {
        Ok(()) => Some((pixel_format, "ChoosePixelFormat fallback")),
        Err(reason) => {
            log::error!(
                "baseview: rejected ChoosePixelFormat pixel format {pixel_format}: {reason}"
            );
            None
        }
    }
}

/// Checks that `pixel_format` can actually back an accelerated OpenGL window, so a format a driver
/// hands back but can't render with is skipped instead of crashing later inside the driver.
unsafe fn validate_pixel_format(
    hdc: HDC, pixel_format: i32, config: &GlConfig,
) -> Result<(), String> {
    let mut pfd: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
    if DescribePixelFormat(
        hdc,
        pixel_format,
        std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u32,
        &mut pfd,
    ) == 0
    {
        return Err(format!("DescribePixelFormat failed (GetLastError = {})", GetLastError()));
    }

    let required = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL;
    if pfd.dwFlags & required != required {
        return Err("cannot draw to a window with OpenGL".into());
    }
    if config.double_buffer && pfd.dwFlags & PFD_DOUBLEBUFFER == 0 {
        return Err("not double-buffered".into());
    }
    if pfd.iPixelType != PFD_TYPE_RGBA {
        return Err("not RGBA".into());
    }
    if pfd.cColorBits < 24 {
        return Err(format!("only {} colour bits", pfd.cColorBits));
    }
    if pfd.dwFlags & PFD_GENERIC_FORMAT != 0 && pfd.dwFlags & PFD_GENERIC_ACCELERATED == 0 {
        return Err("software renderer (Microsoft GDI Generic)".into());
    }
    Ok(())
}
