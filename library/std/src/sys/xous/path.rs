use crate::env;
use crate::ffi::OsStr;
use crate::io;
use crate::mem;
use crate::path::{Path, PathBuf, Prefix};

/// # Safety
///
/// `bytes` must be a valid wtf8 encoded slice
#[inline]
unsafe fn bytes_as_os_str(bytes: &[u8]) -> &OsStr {
    // &OsStr is layout compatible with &Slice, which is compatible with &Wtf8,
    // which is compatible with &[u8].
    unsafe { mem::transmute(bytes) }
}

#[inline]
pub fn is_sep_byte(b: u8) -> bool {
    b == b'/'
}

#[inline]
pub fn is_verbatim_sep(b: u8) -> bool {
    b == b'/'
}

pub fn parse_prefix(prefix: &OsStr) -> Option<Prefix<'_>> {
    let b = prefix.bytes();
    let mut components = b.splitn(2, |x| *x == b':');
    let p = components.next();
    let remainder = components.next();
    if remainder.is_some() {
        Some(Prefix::DeviceNS(unsafe { bytes_as_os_str(p.unwrap()) }))
    } else {
        None
    }
}

pub const MAIN_SEP_STR: &str = "/";
pub const MAIN_SEP: char = '/';

/// Make a POSIX path absolute without changing its semantics.
pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    // This is mostly a wrapper around collecting `Path::components`, with
    // exceptions made where this conflicts with the POSIX specification.
    // See 4.13 Pathname Resolution, IEEE Std 1003.1-2017
    // https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13

    let mut components = path.components();
    let path_os = path.as_os_str().bytes();

    let mut normalized = if path.is_absolute() {
        // "If a pathname begins with two successive <slash> characters, the
        // first component following the leading <slash> characters may be
        // interpreted in an implementation-defined manner, although more than
        // two leading <slash> characters shall be treated as a single <slash>
        // character."
        if path_os.starts_with(b"//") && !path_os.starts_with(b"///") {
            components.next();
            PathBuf::from("//")
        } else {
            PathBuf::new()
        }
    } else {
        env::current_dir()?
    };
    normalized.extend(components);

    // "Interfaces using pathname resolution may specify additional constraints
    // when a pathname that does not name an existing directory contains at
    // least one non- <slash> character and contains one or more trailing
    // <slash> characters".
    // A trailing <slash> is also meaningful if "a symbolic link is
    // encountered during pathname resolution".
    if path_os.ends_with(b"/") {
        normalized.push("");
    }

    Ok(normalized)
}
