mod chapters;
mod database;
mod documents;
mod epub;
mod library;
mod models;

use database::Database;
use library::LibraryWatcher;
use models::*;
use parking_lot::Mutex;
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
async fn document_read(state: State<'_, SharedState>, document_id: String) -> Result<DocumentContent, AppError> {
    documents::read(&state, &document_id)
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

pub fn run() {
    tauri::Builder::default()
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
            library::start_watchers(&app.handle().clone(), &state).ok();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap, app_snapshot, library_add, library_remove, library_refresh,
            document_read, document_write, document_force_write, document_save_as, document_create,
            document_rename, document_move, document_trash, document_update_meta,
            chapter_create, chapter_update, chapter_delete,
            annotation_save, annotation_delete, material_save,
            group_create, group_toggle_document, search, reading_progress_save,
            settings_save, session_save, history_list, history_restore
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Novalyte");
}
