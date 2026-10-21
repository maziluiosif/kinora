//! Keeps the display awake while a video plays, like other video players do.
//! macOS: an IOKit power assertion (shows up in `pmset -g assertions`). Elsewhere: nothing yet.

/// Holds the display awake until dropped.
pub struct KeepAwake {
    #[cfg(target_os = "macos")]
    id: u32,
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};

    pub type CFStringRef = *const c_void;
    pub const UTF8: u32 = 0x0800_0100;
    pub const LEVEL_ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        pub fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
        pub fn IOPMAssertionRelease(id: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> CFStringRef;
        pub fn CFRelease(cf: *const c_void);
    }
}

impl KeepAwake {
    #[cfg(target_os = "macos")]
    pub fn new(reason: &str) -> Option<KeepAwake> {
        use std::ffi::CString;
        let kind = CString::new("PreventUserIdleDisplaySleep").ok()?;
        let reason = CString::new(reason).ok()?;
        // SAFETY: plain CoreFoundation/IOKit calls; both strings are released before returning.
        unsafe {
            let kind = sys::CFStringCreateWithCString(std::ptr::null(), kind.as_ptr(), sys::UTF8);
            let name = sys::CFStringCreateWithCString(std::ptr::null(), reason.as_ptr(), sys::UTF8);
            let mut id = 0;
            let ok = !kind.is_null()
                && !name.is_null()
                && sys::IOPMAssertionCreateWithName(kind, sys::LEVEL_ON, name, &mut id) == 0;
            for s in [kind, name] {
                if !s.is_null() {
                    sys::CFRelease(s);
                }
            }
            if !ok {
                tracing::warn!("could not keep the display awake");
                return None;
            }
            Some(KeepAwake { id })
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn new(_reason: &str) -> Option<KeepAwake> {
        None
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: `id` came from a successful IOPMAssertionCreateWithName.
        unsafe {
            sys::IOPMAssertionRelease(self.id);
        }
    }
}
