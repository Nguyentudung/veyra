//! Minimal compatibility surface for xattr 0.2.3.
//!
//! `zed-async-tar`, pulled by GPUI's HTTP client, only uses [`set`]. The
//! upstream 0.2.3 crate fails on current Linux libc because it references the
//! macOS `ENOATTR` spelling. This implementation keeps that API available and
//! delegates to the native `lsetxattr` call used by the upstream crate.

use std::{
    ffi::{CString, OsStr},
    io,
    os::unix::ffi::OsStrExt,
    path::Path,
};

/// Set an extended attribute without following a symlink, matching xattr
/// 0.2.3's path-oriented API.
pub fn set<N, P>(path: P, name: N, value: &[u8]) -> io::Result<()>
where
    P: AsRef<Path>,
    N: AsRef<OsStr>,
{
    let path = CString::new(path.as_ref().as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "file not found"))?;
    let name = CString::new(name.as_ref().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "name must not contain null bytes",
        )
    })?;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe {
        libc::lsetxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };

    #[cfg(target_os = "macos")]
    let result = unsafe {
        macos_lsetxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            1,
        )
    };

    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    let result = -1;

    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    #[link_name = "lsetxattr"]
    fn macos_lsetxattr(
        path: *const libc::c_char,
        name: *const libc::c_char,
        value: *const libc::c_void,
        size: libc::size_t,
        position: u32,
        options: i32,
    ) -> libc::c_int;
}
