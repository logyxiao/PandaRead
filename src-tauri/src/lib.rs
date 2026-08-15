mod chapters;
mod database;
mod docx;
mod documents;
mod epub;
mod library;
mod models;
mod remote;

use database::Database;
use library::LibraryWatcher;
use models::*;
use parking_lot::Mutex;
use remote::{RemoteManager, RemoteStatus};
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use tauri::{Emitter, Manager, State};

pub struct AppState {
    db: Database,
    watchers: Mutex<Vec<LibraryWatcher>>,
    data_dir: PathBuf,
    initial_scan_started: AtomicBool,
}

type SharedState = Arc<AppState>;

#[tauri::command]
fn bootstrap(app: tauri::AppHandle, state: State<'_, SharedState>) -> Result<AppSnapshot, AppError> {
    let snapshot = state.db.snapshot()?;
    if !state.initial_scan_started.swap(true, Ordering::SeqCst) {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            if library::refresh_all(&state).is_ok() {
                let _ = app.emit("library-changed", ());
            }
        });
    }
    Ok(snapshot)
}

#[tauri::command]
fn app_snapshot(state: State<'_, SharedState>) -> Result<AppSnapshot, AppError> {
    state.db.snapshot()
}

#[tauri::command]
async fn library_add(app: tauri::AppHandle, state: State<'_, SharedState>, path: String) -> Result<AppSnapshot, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        library::add(&state, &path)?;
        library::start_watchers(&app, &state)?;
        state.db.snapshot()
    }).await.map_err(|error| AppError::Message(error.to_string()))?
}

#[tauri::command]
async fn library_remove(app: tauri::AppHandle, state: State<'_, SharedState>, library_id: String) -> Result<AppSnapshot, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.db.remove_library(&library_id)?;
        library::start_watchers(&app, &state)?;
        state.db.snapshot()
    }).await.map_err(|error| AppError::Message(error.to_string()))?
}

#[tauri::command]
async fn library_refresh(state: State<'_, SharedState>) -> Result<AppSnapshot, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        library::refresh_all(&state)?;
        state.db.snapshot()
    }).await.map_err(|error| AppError::Message(error.to_string()))?
}

#[tauri::command]
async fn document_read(state: State<'_, SharedState>, remote: State<'_, Arc<RemoteManager>>, document_id: String) -> Result<DocumentContent, AppError> {
    let content = documents::read(&state, &document_id)?;
    // 双向跟随：桌面切换文稿时推送给所有已连接的手机
    remote.broadcast_desktop_open(&document_id, &content.summary.title);
    Ok(content)
}

#[tauri::command]
async fn document_write(state: State<'_, SharedState>, input: WriteDocumentInput) -> Result<DocumentContent, AppError> {
    documents::write(&state, input)
}

#[tauri::command]
async fn document_force_write(state: State<'_, SharedState>, input: WriteDocumentInput) -> Result<DocumentContent, AppError> {
    documents::force_write(&state, input)
}

#[tauri::command]
fn document_save_as(state: State<'_, SharedState>, document_id: String, content: String, target_path: String) -> Result<(), AppError> {
    documents::save_as(&state, &document_id, &content, &target_path)
}

#[tauri::command]
async fn document_tag_update(state: State<'_, SharedState>, document_id: String, tags: Vec<String>) -> Result<DocumentSummary, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.db.update_tags(&document_id, &tags)).await.map_err(|e| AppError::Message(e.to_string()))?
}

#[tauri::command]
async fn document_tidy(state: State<'_, SharedState>, document_id: String) -> Result<DocumentContent, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || documents::tidy(&state, &document_id)).await.map_err(|e| AppError::Message(e.to_string()))?
}

#[tauri::command]
fn document_create(state: State<'_, SharedState>, input: FileCreateInput) -> Result<AppSnapshot, AppError> {
    library::create_entry(&state, input)?;
    state.db.snapshot()
}

#[tauri::command]
fn document_rename(state: State<'_, SharedState>, input: FileRenameInput) -> Result<AppSnapshot, AppError> {
    library::rename_entry(&state, input)?;
    state.db.snapshot()
}

#[tauri::command]
fn document_move(state: State<'_, SharedState>, input: FileMoveInput) -> Result<AppSnapshot, AppError> {
    library::move_entry(&state, input)?;
    state.db.snapshot()
}

#[tauri::command]
fn document_trash(state: State<'_, SharedState>, input: FileTargetInput) -> Result<AppSnapshot, AppError> {
    library::trash_entry(&state, input)?;
    state.db.snapshot()
}

#[tauri::command]
fn document_update_meta(state: State<'_, SharedState>, input: DocumentMetaInput) -> Result<AppSnapshot, AppError> {
    state.db.update_document_meta(input)?;
    state.db.snapshot()
}

#[tauri::command]
fn document_shelf(state: State<'_, SharedState>, document_id: String, shelf: String) -> Result<DocumentSummary, AppError> {
    state.db.update_shelf(&document_id, &shelf)
}

/// 批量返回文档前几段预览（卡片视图用），单篇失败跳过不影响其余。
#[tauri::command]
async fn document_previews(state: State<'_, SharedState>, ids: Vec<String>) -> Result<Vec<serde_json::Value>, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for id in ids.iter().take(500) {
            if let Ok(paragraphs) = documents::preview(&state, id, 3, 60) {
                out.push(serde_json::json!({ "documentId": id, "paragraphs": paragraphs }));
            }
        }
        Ok(out)
    }).await.map_err(|error| AppError::Message(error.to_string()))?
}

#[tauri::command]
fn chapter_create(state: State<'_, SharedState>, input: ChapterInput) -> Result<Vec<ChapterNode>, AppError> {
    state.db.create_chapter(input)
}

#[tauri::command]
fn chapter_update(state: State<'_, SharedState>, input: ChapterUpdateInput) -> Result<Vec<ChapterNode>, AppError> {
    state.db.update_chapter(input)
}

#[tauri::command]
fn chapter_delete(state: State<'_, SharedState>, chapter_id: String, document_id: String) -> Result<Vec<ChapterNode>, AppError> {
    state.db.delete_chapter(&chapter_id, &document_id)
}

#[tauri::command]
fn annotation_save(state: State<'_, SharedState>, input: AnnotationInput) -> Result<Vec<Annotation>, AppError> {
    state.db.save_annotation(input)
}

#[tauri::command]
fn annotation_delete(state: State<'_, SharedState>, annotation_id: String, document_id: String) -> Result<Vec<Annotation>, AppError> {
    state.db.delete_annotation(&annotation_id, &document_id)
}

#[tauri::command]
fn material_save(state: State<'_, SharedState>, input: MaterialInput) -> Result<Vec<MaterialClip>, AppError> {
    state.db.save_material(input)
}

#[tauri::command]
fn group_create(state: State<'_, SharedState>, name: String) -> Result<AppSnapshot, AppError> {
    state.db.create_group(&name)?;
    state.db.snapshot()
}

#[tauri::command]
fn group_toggle_document(state: State<'_, SharedState>, group_id: String, document_id: String) -> Result<AppSnapshot, AppError> {
    state.db.toggle_group_document(&group_id, &document_id)?;
    state.db.snapshot()
}

#[tauri::command]
fn search(state: State<'_, SharedState>, query: SearchQuery) -> Result<Vec<SearchResult>, AppError> {
    state.db.search(query)
}

#[tauri::command]
fn reading_progress_save(state: State<'_, SharedState>, input: ReadingProgress) -> Result<(), AppError> {
    state.db.save_progress(input)
}

#[tauri::command]
fn settings_save(state: State<'_, SharedState>, settings: UserSettings) -> Result<(), AppError> {
    state.db.save_settings(&settings)
}

#[tauri::command]
fn session_save(state: State<'_, SharedState>, session: AppSession) -> Result<(), AppError> {
    state.db.save_session(&session)
}

#[tauri::command]
fn history_list(state: State<'_, SharedState>, document_id: String) -> Result<Vec<HistoryEntry>, AppError> {
    state.db.history(&document_id)
}

#[tauri::command]
fn history_restore(state: State<'_, SharedState>, history_id: String, document_id: String) -> Result<DocumentContent, AppError> {
    documents::restore_history(&state, &history_id, &document_id)
}

/// 导出文稿为 Word（宋体小四、1.5 倍行距、###N. 章节转第N章），
/// 输出到源文件所在目录，文件名与文档顶部标题取小说标题（正文.md 用父文件夹名）。
#[tauri::command]
async fn document_export_docx(state: State<'_, SharedState>, document_id: String) -> Result<String, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || docx::export(&state, &document_id))
        .await
        .map_err(|error| AppError::Message(error.to_string()))?
}

#[tauri::command]
fn remote_start(remote: State<'_, Arc<RemoteManager>>) -> Result<RemoteStatus, AppError> {
    remote.start()
}

#[tauri::command]
fn remote_stop(remote: State<'_, Arc<RemoteManager>>) -> Result<RemoteStatus, AppError> {
    remote.stop();
    Ok(remote.status())
}

#[tauri::command]
fn remote_status(remote: State<'_, Arc<RemoteManager>>) -> Result<RemoteStatus, AppError> {
    Ok(remote.status())
}

#[tauri::command]
fn remote_tunnel_start(remote: State<'_, Arc<RemoteManager>>) -> Result<(), AppError> {
    remote.tunnel_start()
}

#[tauri::command]
fn remote_tunnel_stop(remote: State<'_, Arc<RemoteManager>>) -> Result<(), AppError> {
    remote.tunnel_stop();
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&data_dir)?;
            let state = Arc::new(AppState {
                db: Database::open(data_dir.join("novalyte.sqlite3")).map_err(|e| e.to_string())?,
                watchers: Mutex::new(Vec::new()),
                data_dir,
                initial_scan_started: AtomicBool::new(false),
            });
            app.manage(state.clone());
            app.manage(Arc::new(RemoteManager::new(app.handle().clone(), state.clone())));
            library::start_watchers(&app.handle().clone(), &state).ok();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap, app_snapshot, library_add, library_remove, library_refresh,
            document_read, document_write, document_force_write, document_save_as, document_tidy, document_create,
            document_rename, document_move, document_trash, document_update_meta, document_shelf, document_previews,
            chapter_create, chapter_update, chapter_delete,
            annotation_save, annotation_delete, material_save,
            document_tag_update,
            group_create, group_toggle_document, search, reading_progress_save,
            settings_save, session_save, history_list, history_restore, document_export_docx,
            remote_start, remote_stop, remote_status, remote_tunnel_start, remote_tunnel_stop
        ])
        .build(tauri::generate_context!())
        .expect("failed to build 熊猫阅读");
    app.run(|app_handle, event| {
        // 退出时停止手机阅读服务与公网隧道，释放端口和子进程
        if let tauri::RunEvent::Exit = event {
            if let Some(remote) = app_handle.try_state::<Arc<RemoteManager>>() {
                remote.stop_all();
            }
        }
    });
}
