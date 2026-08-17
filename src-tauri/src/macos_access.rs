//! Persist macOS folder authorization with security-scoped bookmarks.
//! Selecting a library once should keep access across launches.

#[cfg(not(target_os = "macos"))]
use std::path::{Path, PathBuf};

pub struct ScopedAccess {
    #[cfg(target_os = "macos")]
    _url: objc2::rc::Retained<objc2_foundation::NSURL>,
}

#[cfg(not(target_os = "macos"))]
pub fn bookmark_for_path(_path: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn start_access(_bookmark: &[u8]) -> Option<(PathBuf, ScopedAccess, bool)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn start_path(_path: &Path) -> Option<ScopedAccess> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn has_full_disk_access() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn open_full_disk_access_settings() -> std::io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
mod imp {
    use super::ScopedAccess;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSString, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions, NSURL,
    };
    use std::{
        fs::File,
        path::{Path, PathBuf},
        process::Command,
    };

    fn file_url(path: &Path) -> Option<Retained<NSURL>> {
        Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
    }

    pub fn bookmark_for_path(path: &Path) -> Option<Vec<u8>> {
        let url = file_url(path)?;
        let data = url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::WithSecurityScope,
                None,
                None,
            )
            .ok()?;
        Some(data.to_vec())
    }

    pub fn start_access(bookmark: &[u8]) -> Option<(PathBuf, ScopedAccess, bool)> {
        let data = NSData::with_bytes(bookmark);
        let mut stale = Bool::NO;
        let url = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &data,
                NSURLBookmarkResolutionOptions::WithSecurityScope,
                None,
                &mut stale,
            )
        }
        .ok()?;
        unsafe {
            let _ = url.startAccessingSecurityScopedResource();
        }
        let path = PathBuf::from(url.path()?.to_string());
        Some((path, ScopedAccess { _url: url }, stale.as_bool()))
    }

    pub fn start_path(path: &Path) -> Option<ScopedAccess> {
        let url = file_url(path)?;
        unsafe {
            let _ = url.startAccessingSecurityScopedResource();
        }
        Some(ScopedAccess { _url: url })
    }

    pub fn has_full_disk_access() -> bool {
        let Some(user_home) = std::env::var_os("HOME") else {
            return false;
        };
        File::open(
            PathBuf::from(user_home).join("Library/Application Support/com.apple.TCC/TCC.db"),
        )
        .is_ok()
    }

    pub fn open_full_disk_access_settings() -> std::io::Result<()> {
        let status = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("无法打开系统设置"))
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    bookmark_for_path, has_full_disk_access, open_full_disk_access_settings, start_access,
    start_path,
};
