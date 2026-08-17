use crate::{database::ScanEntry, documents::safe_path, macos_access, models::*, AppState};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

pub struct LibraryWatcher {
    _watcher: RecommendedWatcher,
}

pub fn add(state: &Arc<AppState>, path: &str) -> Result<(), AppError> {
    let selected = PathBuf::from(path);
    let root = if selected.is_file() {
        selected.parent().ok_or(AppError::NotFound)?.to_path_buf()
    } else {
        selected
    };
    // Refuse nested roots: an inner folder would be indexed twice and double-scan on every change.
    let canonical = root.canonicalize().map_err(|_| AppError::NotFound)?;
    for (_id, existing, _) in state.db.library_paths()? {
        if canonical.starts_with(&existing) && canonical != existing {
            return Err(AppError::Message(format!(
                "「{}」已包含在已有书库中，无需重复导入",
                canonical
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("该文件夹")
            )));
        }
        if existing.starts_with(&canonical) && existing != canonical {
            return Err(AppError::Message(format!(
                "「{}」包含已有书库，请直接导入其上级文件夹",
                canonical
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("该文件夹")
            )));
        }
    }
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("本地书库");
    let bookmark = macos_access::bookmark_for_path(&canonical);
    if let Some(access) = bookmark
        .as_deref()
        .and_then(macos_access::start_access)
        .map(|(_, access, _)| access)
        .or_else(|| macos_access::start_path(&canonical))
    {
        state.scoped_access.lock().push(access);
    }
    let id = state
        .db
        .add_library_with_bookmark(&root, name, bookmark.as_deref())?;
    scan(state, &id, &root)
}

pub fn restore_access(state: &Arc<AppState>) {
    let Ok(rows) = state.db.library_bookmarks() else {
        return;
    };
    let mut held = state.scoped_access.lock();
    for (id, root, bookmark) in rows {
        if let Some(bytes) = bookmark.as_deref() {
            if let Some((resolved, access, stale)) = macos_access::start_access(bytes) {
                if stale {
                    if let Some(fresh) = macos_access::bookmark_for_path(&resolved) {
                        let _ = state.db.set_library_bookmark(&id, &fresh);
                    }
                }
                held.push(access);
                continue;
            }
        }
        if let Some(fresh) = macos_access::bookmark_for_path(&root) {
            if let Some((_, access, _)) = macos_access::start_access(&fresh) {
                let _ = state.db.set_library_bookmark(&id, &fresh);
                held.push(access);
                continue;
            }
        }
        if let Some(access) = macos_access::start_path(&root) {
            held.push(access);
        }
    }
}
pub fn refresh_all(state: &Arc<AppState>) -> Result<(), AppError> {
    for (id, root, _) in state.db.library_paths()? {
        scan(state, &id, &root)?;
    }
    Ok(())
}
fn scan(state: &Arc<AppState>, library_id: &str, root: &Path) -> Result<(), AppError> {
    let known = state.db.document_mtimes(library_id)?;
    let mut entries: Vec<ScanEntry> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    let mut complete = true;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !hidden(e.path(), root))
    {
        let e = match entry {
            Ok(e) => e,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let relative = e
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::PathOutsideLibrary)?
            .to_string_lossy()
            .replace('\\', "/");
        if e.file_type().is_dir() {
            if !relative.is_empty() {
                folders.push(relative);
            }
            continue;
        }
        if !e.file_type().is_file() {
            continue;
        }
        let ext = e
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext != "txt"
            && ext != "epub"
            && !(ext == "md"
                && e.path().file_stem().and_then(|s| s.to_str()).unwrap_or("") == "正文")
        {
            continue;
        }
        let title = e
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("未命名")
            .to_string();
        let modified = crate::documents::file_stamp(e.path());
        seen.push(relative.clone());
        if known.get(&relative) == Some(&modified) {
            continue;
        }
        let tax = taxonomy(root, &relative);
        entries.push(ScanEntry {
            library_id: library_id.into(),
            relative_path: relative,
            title,
            format: ext,
            word_count: 0,
            modified_at: modified,
            hash: String::new(),
            encoding: "utf-8".into(),
            newline: "\n".into(),
            taxonomy: tax,
        });
    }
    state.db.upsert_documents(&entries)?;
    if !complete {
        return Err(AppError::Message(format!(
            "「{}」中有部分目录无法读取，已保留原有索引",
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("书库")
        )));
    }
    state.db.update_missing_flags(library_id, &seen)?;
    state.db.replace_library_folders(library_id, &folders)?;
    Ok(())
}
fn hidden(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    path.file_name()
        .and_then(|s| s.to_str())
        .is_none_or(|s| s.starts_with('.') || s == "__MACOSX" || s == "node_modules")
}
fn taxonomy(root: &Path, relative: &str) -> [String; 3] {
    let mut parts: Vec<String> = relative.split('/').map(str::to_string).collect();
    parts.pop();
    let root_name = root.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if root_name == "男频" || root_name == "女频" {
        parts.insert(0, root_name.into());
    }
    let gender_pos = parts.iter().position(|p| p == "男频" || p == "女频");
    if let Some(i) = gender_pos {
        [
            parts.get(i).cloned().unwrap_or_else(|| "未分类".into()),
            parts.get(i + 1).cloned().unwrap_or_else(|| "未分类".into()),
            parts.get(i + 2).cloned().unwrap_or_else(|| "未分类".into()),
        ]
    } else {
        [
            "未分类".into(),
            parts.first().cloned().unwrap_or_else(|| "未分类".into()),
            parts.get(1).cloned().unwrap_or_else(|| "未分类".into()),
        ]
    }
}

pub fn start_watchers(app: &AppHandle, state: &Arc<AppState>) -> Result<(), AppError> {
    let mut list = state.watchers.lock();
    list.clear();
    for (_, root, _) in state.db.library_paths()? {
        let app = app.clone();
        let state = state.clone();
        // Coalesce bursts: at most one scan worker per watcher; events while scanning just mark pending.
        let scanning = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicBool::new(false));
        let (scanning_cb, pending_cb) = (scanning.clone(), pending.clone());
        let mut watcher =
            notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                let Ok(event) = event else { return };
                let changed = event.paths.into_iter().next().unwrap_or_default();
                pending_cb.store(true, Ordering::Relaxed);
                if scanning_cb.swap(true, Ordering::Relaxed) {
                    return;
                }
                let app = app.clone();
                let state = state.clone();
                let s = scanning_cb.clone();
                let p = pending_cb.clone();
                std::thread::spawn(move || loop {
                    p.store(false, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(300));
                    let _ = scan_changed(&state, &changed);
                    let _ = app.emit("library-changed", ());
                    if !p.load(Ordering::Relaxed) {
                        s.store(false, Ordering::Relaxed);
                        break;
                    }
                });
            })
            .map_err(|e| AppError::Message(e.to_string()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| AppError::Message(e.to_string()))?;
        list.push(LibraryWatcher { _watcher: watcher });
    }
    Ok(())
}

// Scan only the library containing the changed path (deepest root wins) instead of every library.
fn scan_changed(state: &Arc<AppState>, changed: &Path) -> Result<(), AppError> {
    let mut matched: Option<(String, PathBuf)> = None;
    for (id, root, _) in state.db.library_paths()? {
        if changed.starts_with(&root) && matched.as_ref().is_none_or(|(_, r)| root.starts_with(r)) {
            matched = Some((id, root));
        }
    }
    match matched {
        Some((id, root)) => scan(state, &id, &root),
        None => refresh_all(state),
    }
}

pub fn create_entry(state: &Arc<AppState>, i: FileCreateInput) -> Result<(), AppError> {
    let (_, root, _) = find_library(state, &i.library_id)?;
    let parent = if i.parent_path.is_empty() {
        root.clone()
    } else {
        safe_path(&root, &i.parent_path)?
    };
    let name = clean_name(&i.name)?;
    let path = parent.join(
        if i.kind == "file" && !name.to_lowercase().ends_with(".txt") {
            format!("{}.txt", name)
        } else {
            name
        },
    );
    if path.exists() {
        return Err(AppError::AlreadyExists);
    }
    if i.kind == "folder" {
        fs::create_dir(&path)?;
    } else {
        fs::write(&path, b"")?;
    }
    scan(state, &i.library_id, &root)
}
pub fn rename_entry(state: &Arc<AppState>, i: FileRenameInput) -> Result<(), AppError> {
    let (_, root, _) = find_library(state, &i.library_id)?;
    let source = safe_path(&root, &i.relative_path)?;
    let name = clean_name(&i.new_name)?;
    let ext = source.extension().and_then(|s| s.to_str());
    let target_name = if source.is_file() && Path::new(&name).extension().is_none() {
        format!("{}.{}", name, ext.unwrap_or("txt"))
    } else {
        name
    };
    let target = source
        .parent()
        .ok_or(AppError::PathOutsideLibrary)?
        .join(target_name);
    if target.exists() {
        return Err(AppError::AlreadyExists);
    }
    let target_relative = target
        .strip_prefix(&root)
        .map_err(|_| AppError::PathOutsideLibrary)?
        .to_string_lossy()
        .replace('\\', "/");
    fs::rename(&source, &target)?;
    if let Err(error) = state.db.relocate_documents(
        &i.library_id,
        &i.relative_path,
        &i.library_id,
        &target_relative,
    ) {
        let _ = fs::rename(&target, &source);
        return Err(error);
    }
    scan(state, &i.library_id, &root)
}
pub fn move_entry(state: &Arc<AppState>, i: FileMoveInput) -> Result<(), AppError> {
    let (_, src_root, _) = find_library(state, &i.source_library_id)?;
    let (_, dst_root, _) = find_library(state, &i.target_library_id)?;
    let source = safe_path(&src_root, &i.relative_path)?;
    let parent = if i.target_parent_path.is_empty() {
        dst_root.clone()
    } else {
        safe_path(&dst_root, &i.target_parent_path)?
    };
    let target = parent.join(source.file_name().ok_or(AppError::PathOutsideLibrary)?);
    if target.exists() {
        return Err(AppError::AlreadyExists);
    }
    let target_relative = target
        .strip_prefix(&dst_root)
        .map_err(|_| AppError::PathOutsideLibrary)?
        .to_string_lossy()
        .replace('\\', "/");
    if src_root == dst_root {
        fs::rename(&source, &target)?;
    } else {
        copy_verified(&source, &target)?;
        trash::delete(&source).map_err(|e| AppError::Message(e.to_string()))?;
    }
    state.db.relocate_documents(
        &i.source_library_id,
        &i.relative_path,
        &i.target_library_id,
        &target_relative,
    )?;
    scan(state, &i.source_library_id, &src_root)?;
    if i.source_library_id != i.target_library_id {
        scan(state, &i.target_library_id, &dst_root)?;
    }
    Ok(())
}
pub fn trash_entry(state: &Arc<AppState>, i: FileTargetInput) -> Result<(), AppError> {
    let (_, root, _) = find_library(state, &i.library_id)?;
    let path = safe_path(&root, &i.relative_path)?;
    trash::delete(path).map_err(|e| AppError::Message(e.to_string()))?;
    scan(state, &i.library_id, &root)
}
pub fn reveal_entry(state: &Arc<AppState>, i: FileTargetInput) -> Result<(), AppError> {
    let (_, root, _) = find_library(state, &i.library_id)?;
    let path = if i.relative_path.is_empty() {
        root
    } else {
        safe_path(&root, &i.relative_path)?
    };
    if !path.exists() {
        return Err(AppError::NotFound);
    }
    reveal_in_file_manager(&path)
}
fn reveal_in_file_manager(path: &Path) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .status()
            .map_err(|e| AppError::Message(e.to_string()))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .status()
            .map_err(|e| AppError::Message(e.to_string()))?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        std::process::Command::new("xdg-open")
            .arg(dir)
            .status()
            .map_err(|e| AppError::Message(e.to_string()))?;
    }
    Ok(())
}
fn copy_verified(src: &Path, dst: &Path) -> Result<(), AppError> {
    if src.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_verified(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        fs::copy(src, dst)?;
        let a = hash_file(src)?;
        let b = hash_file(dst)?;
        if a != b {
            return Err(AppError::Message("跨书库复制校验失败，原文件未移动".into()));
        }
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<Vec<u8>, AppError> {
    let bytes = fs::read(path)?;
    Ok(Sha256::digest(bytes).to_vec())
}
fn find_library(state: &Arc<AppState>, id: &str) -> Result<(String, PathBuf, String), AppError> {
    state
        .db
        .library_paths()?
        .into_iter()
        .find(|x| x.0 == id)
        .ok_or(AppError::NotFound)
}
fn clean_name(name: &str) -> Result<String, AppError> {
    let n = name.trim();
    if n.is_empty()
        || n == "."
        || n == ".."
        || n.contains('/')
        || n.contains('\\')
        || n.contains(':')
    {
        return Err(AppError::Message("名称包含无效字符".into()));
    }
    Ok(n.into())
}
