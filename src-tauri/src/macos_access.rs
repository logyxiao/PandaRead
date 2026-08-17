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

#[cfg(target_os = "macos")]
mod imp {
    use super::ScopedAccess;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSString, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions, NSURL,
    };
    use std::path::{Path, PathBuf};

    fn file_url(path: &Path) -> Option<Retained<NSURL>> {
        Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
    }

    pub fn bookmark_for_path(path: &Path) -> Option<Vec<u8>> {
        let url = file_url(path)?;
        for options in [
            NSURLBookmarkCreationOptions::WithSecurityScope,
            NSURLBookmarkCreationOptions::empty(),
        ] {
            if let Ok(data) = url
                .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                    options,
                    None,
                    None,
                )
            {
                return Some(data.to_vec());
            }
        }
        None
    }

    pub fn start_access(bookmark: &[u8]) -> Option<(PathBuf, ScopedAccess, bool)> {
        let data = NSData::with_bytes(bookmark);
        for options in [
            NSURLBookmarkResolutionOptions::WithSecurityScope,
            NSURLBookmarkResolutionOptions::empty(),
        ] {
            let mut stale = Bool::NO;
            let Ok(url) = (unsafe {
                NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                    &data,
                    options,
                    None,
                    &mut stale,
                )
            }) else {
                continue;
            };
            if options.contains(NSURLBookmarkResolutionOptions::WithSecurityScope) {
                unsafe {
                    let _ = url.startAccessingSecurityScopedResource();
                }
            }
            let path = PathBuf::from(url.path()?.to_string());
            return Some((path, ScopedAccess { _url: url }, stale.as_bool()));
        }
        None
    }

    pub fn start_path(path: &Path) -> Option<ScopedAccess> {
        let url = file_url(path)?;
        unsafe {
            let _ = url.startAccessingSecurityScopedResource();
        }
        Some(ScopedAccess { _url: url })
    }
}

#[cfg(target_os = "macos")]
pub use imp::{bookmark_for_path, start_access, start_path};
