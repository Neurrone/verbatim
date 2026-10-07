//! Common Controls version 6 for mockapp's real controls: its manifest
//! (`mockapp.exe.manifest`, embedded as resource 2 by `build.rs`) as an
//! activation context, active while a control is made, so the process
//! otherwise keeps the classic controls: its edit control is the classic
//! one unless the fixture asks for version 6's.

use windows::Win32::Foundation::{HANDLE, HMODULE, INVALID_HANDLE_VALUE};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::core::PCWSTR;

/// The layout of `ACTCTXW`, declared here so mockapp needs no further
/// feature of the `windows` crate.
#[repr(C)]
struct ActCtx {
    size: u32,
    flags: u32,
    source: PCWSTR,
    processor_architecture: u16,
    language: u16,
    assembly_directory: PCWSTR,
    resource_name: PCWSTR,
    application_name: PCWSTR,
    module: HMODULE,
}

/// `ACTCTX_FLAG_RESOURCE_NAME_VALID`.
const RESOURCE_NAME_VALID: u32 = 0x8;

/// `ACTCTX_FLAG_HMODULE_VALID`.
const HMODULE_VALID: u32 = 0x80;

/// The manifest's resource id in mockapp's executable (`build.rs`).
const MANIFEST_RESOURCE: usize = 2;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateActCtxW(context: *const ActCtx) -> HANDLE;
    fn ActivateActCtx(context: HANDLE, cookie: *mut usize) -> i32;
    fn DeactivateActCtx(flags: u32, cookie: usize) -> i32;
    fn ReleaseActCtx(context: HANDLE);
}

/// Common Controls version 6, active on this thread while this lives:
/// mockapp's manifest (`mockapp.exe.manifest`, resource 2) as an
/// activation context. The tree view and the list view are made in it,
/// and the edit control when the fixture asks; the rest of the process
/// keeps the classic controls.
pub(crate) struct CommonControls6 {
    context: HANDLE,
    cookie: usize,
}

impl CommonControls6 {
    pub(crate) fn activate() -> windows::core::Result<Self> {
        let mut path = [0u16; 1024];
        // SAFETY: writes at most the local buffer's length.
        let length = unsafe { GetModuleFileNameW(None, &mut path) } as usize;
        if length == 0 || length >= path.len() {
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: retrieves this module's own instance handle.
        let module = unsafe { GetModuleHandleW(None) }?;
        let request = ActCtx {
            size: u32::try_from(size_of::<ActCtx>()).unwrap_or(0),
            flags: RESOURCE_NAME_VALID | HMODULE_VALID,
            source: PCWSTR(path.as_ptr()),
            processor_architecture: 0,
            language: 0,
            assembly_directory: PCWSTR::null(),
            // A resource id, as `MAKEINTRESOURCE` makes one.
            resource_name: PCWSTR(MANIFEST_RESOURCE as *const u16),
            application_name: PCWSTR::null(),
            module,
        };
        // SAFETY: `request` is fully initialized, and the path it names
        // outlives the call.
        let context = unsafe { CreateActCtxW(&raw const request) };
        if context == INVALID_HANDLE_VALUE {
            return Err(windows::core::Error::from_thread());
        }
        let mut cookie = 0usize;
        // SAFETY: a context just created, and a local cookie.
        if unsafe { ActivateActCtx(context, &raw mut cookie) } == 0 {
            let error = windows::core::Error::from_thread();
            // SAFETY: the context created above, released once.
            unsafe { ReleaseActCtx(context) };
            return Err(error);
        }
        Ok(Self { context, cookie })
    }
}

impl Drop for CommonControls6 {
    fn drop(&mut self) {
        // SAFETY: the cookie `activate` made, on its thread.
        unsafe { DeactivateActCtx(0, self.cookie) };
        // SAFETY: the context `activate` made, released once.
        unsafe { ReleaseActCtx(self.context) };
    }
}
