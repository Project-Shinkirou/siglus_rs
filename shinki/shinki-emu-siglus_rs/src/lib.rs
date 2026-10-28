//! Shinki C boundary. Native VM, render, audio and save remain in siglus_scene_vm.
use shinki_emu_abi as abi;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, Ordering},
};
mod beta_saves;
mod configuration;
mod frames;
mod input;
mod platform;
mod session;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
type Result<T, E = Failure> = std::result::Result<T, E>;
#[derive(Clone, Debug)]
struct Failure {
    status: abi::Status,
    code: &'static str,
    message: String,
}
impl Failure {
    fn new(status: abi::Status, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn native(error: impl std::fmt::Display) -> Self {
        Self::new(
            abi::CORE_ERROR,
            "SHINKI_EMU_SIGLUS_ENGINE",
            format!("{error:#}"),
        )
    }
    fn error(&self) -> abi::Error {
        let mut e = abi::Error::default();
        e.set(self.code, &self.message);
        e
    }
}
impl From<abi::Status> for Failure {
    fn from(status: abi::Status) -> Self {
        Self::new(
            status,
            "SHINKI_EMU_SIGLUS_ARGUMENT",
            "invalid or unsupported ABI request",
        )
    }
}
fn boundary(error: *mut abi::Error, f: impl FnOnce() -> Result<()>) -> abi::Status {
    let result = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(Failure::new(
            abi::PANICKED,
            "SHINKI_EMU_SIGLUS_PANIC",
            "native boundary panicked",
        ))
    });
    match result {
        Ok(()) => {
            if !error.is_null() {
                unsafe { *error = abi::Error::default() };
            }
            abi::OK
        }
        Err(failure) => {
            if !error.is_null() {
                unsafe { *error = failure.error() };
            }
            failure.status
        }
    }
}
fn initialized() -> Result<()> {
    if INITIALIZED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(abi::INVALID_STATE.into())
    }
}
unsafe extern "C" fn initialize(
    diagnostics: *const abi::Diagnostics,
    error: *mut abi::Error,
) -> abi::Status {
    boundary(error, || {
        let d = unsafe { diagnostics.as_ref() }.ok_or(abi::INVALID_ARGUMENT)?;
        if !d.header.accepts::<abi::Diagnostics>() || d.emit.is_none() || d.flush.is_none() {
            return Err(abi::INVALID_ARGUMENT.into());
        }
        #[cfg(feature = "dynamic")]
        unsafe { abi::diagnostic_bridge::install(*d) }.map_err(|status| {
            Failure::new(
                status,
                "SHINKI_EMU_FAMILY_DIAGNOSTIC_INIT",
                "diagnostic bridge initialization failed",
            )
        })?;
        INITIALIZED.store(true, Ordering::Release);
        Ok(())
    })
}
static API: abi::CoreApi = abi::CoreApi {
    header: abi::Header::of::<abi::CoreApi>(),
    initialize: Some(initialize),
    describe: Some(configuration::describe),
    probe: Some(configuration::probe),
    open: Some(session::open),
    input: Some(session::input),
    control: Some(session::control),
    state: Some(session::state),
    frame: Some(session::frame),
    cancel: Some(session::cancel),
    close: Some(session::close),
    extension: Some(extension),
};
unsafe extern "C" fn extension(
    _: abi::Bytes,
    _: u32,
    out: *mut *const std::ffi::c_void,
) -> abi::Status {
    if out.is_null() {
        return abi::INVALID_ARGUMENT;
    }
    unsafe { *out = std::ptr::null() };
    abi::UNSUPPORTED
}
/// Static registration and dynamic export use this exact table.
///
/// # Safety
/// A non-null `out` must point to writable storage for one core-table pointer.
pub unsafe extern "C" fn query(version: u32, out: *mut *const abi::CoreApi) -> abi::Status {
    if out.is_null() {
        return abi::INVALID_ARGUMENT;
    }
    unsafe { *out = std::ptr::null() };
    if version != abi::ABI_VERSION {
        return abi::UNSUPPORTED;
    }
    unsafe { *out = &API };
    abi::OK
}

/// Dynamic export of [`query`]; both return the same native core table.
///
/// # Safety
/// A non-null `out` must point to writable storage for one core-table pointer.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shinki_emu_query(
    version: u32,
    out: *mut *const abi::CoreApi,
) -> abi::Status {
    unsafe { query(version, out) }
}

#[cfg(test)]
#[path = "boundary_tests.rs"]
mod tests;
