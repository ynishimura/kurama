//! Environment writes from unit tests. `std::env::set_var` is unsafe since
//! edition 2024 -- another thread reading the environment while it changes is
//! undefined -- and the argument that makes it sound is the same at every call
//! site, so it is made once, here, instead of forty times.

use std::ffi::OsStr;

/// # Safety argument
/// Every caller is a `#[serial_test::serial]` test, so no other test thread
/// runs while the variable is set, and production code reads the environment
/// only through the adapters those tests drive.
pub(crate) fn set(key: &str, value: impl AsRef<OsStr>) {
    unsafe { std::env::set_var(key, value) }
}

/// Removes a variable under the same argument as [`set`].
pub(crate) fn remove(key: &str) {
    unsafe { std::env::remove_var(key) }
}

/// Sets a variable, or removes it when there is no value: what
/// [`std::env::var_os`] returned before a test changed it restores as it is.
pub(crate) fn set_or_remove(key: &str, value: Option<impl AsRef<OsStr>>) {
    match value {
        Some(value) => set(key, value),
        None => remove(key),
    }
}
