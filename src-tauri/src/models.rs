use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("文件或目录不存在")]
    NotFound,
    #[error("目标路径不在已授权书库内")]
    PathOutsideLibrary,
    #[error("同名文件或目录已经存在")]
    AlreadyExists,
    #[error("文稿已被其他程序修改")]
    WriteConflict,
    #[error("{0}")]
    Message(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        let (code, message) = match self {
            Self::NotFound => ("NOT_FOUND", self.to_string()),
            Self::PathOutsideLibrary => ("PATH_OUTSIDE_LIBRARY", self.to_string()),
            Self::AlreadyExists => ("ALREADY_EXISTS", self.to_string()),
            Self::WriteConflict => ("WRITE_CONFLICT", self.to_string()),
            Self::Message(_) => ("APP_ERROR", self.to_string()),
        };
        ErrorPayload { code, message: &message }.serialize(serializer)
    }
}

#[derive(Serialize)]
struct ErrorPayload<'a> { code: &'a str, message: &'a str }

impl From<std::io::Error> for AppError { fn from(e: std::io::Error) -> Self { Self::Message(e.to_string()) } }
impl From<rusqlite::Error> for AppError { fn from(e: rusqlite::Error) -> Self { Self::Message(e.to_string()) } }
impl From<zip::result::ZipError> for AppError { fn from(e: zip::result::ZipError) -> Self { Self::Message(e.to_string()) } }
impl From<walkdir::Error> for AppError { fn from(e: walkdir::Error) -> Self { Self::Message(e.to_string()) } }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRoot { pub id: String, pub name: String, pub document_count: i64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String, pub library_id: String, pub relative_path: String, pub title: String,
    pub format: String, pub word_count: i64, pub modified_at: i64, pub gender: String,
    pub genre: String, pub subgenre: String, pub length_kind: String, pub purpose: String,
    pub progress: String, pub favorite: bool, pub missing: bool, pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode { pub name: String, pub relative_path: String, pub kind: String, pub library_id: String, pub document_id: Option<String>, pub count: i64, pub children: Vec<TreeNode> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterNode { pub id: String, pub document_id: String, pub title: String, pub offset: i64, pub kind: String, pub level: i64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation { pub id: String, pub document_id: String, pub start_offset: i64, pub end_offset: i64, pub quote: String, pub prefix: String, pub suffix: String, pub note: String, pub marker: String, pub orphaned: bool, pub created_at: i64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialClip { pub id: String, pub document_id: String, pub quote: String, pub note: String, pub group_name: String, pub source_title: String, pub start_offset: i64, pub created_at: i64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualGroup { pub id: String, pub name: String, pub document_ids: Vec<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingProgress { pub document_id: String, pub chapter_id: Option<String>, pub char_offset: i64, pub scroll_ratio: f64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderSettings { pub theme: String, pub font_family: String, pub font_size: f64, pub letter_spacing: f64, pub line_height: f64, pub paper_width: f64, pub page_margin: f64, pub speech_rate: f64 }

impl Default for ReaderSettings { fn default() -> Self { Self { theme: "day".into(), font_family: "serif".into(), font_size: 18.0, letter_spacing: 0.0, line_height: 1.95, paper_width: 760.0, page_margin: 56.0, speech_rate: 1.0 } } }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings { pub reader: ReaderSettings, pub shortcuts: std::collections::HashMap<String, String>, pub always_on_top: bool }
impl Default for UserSettings { fn default() -> Self { Self { reader: ReaderSettings::default(), shortcuts: std::collections::HashMap::new(), always_on_top: false } } }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSession { pub left_document_id: Option<String>, pub right_document_id: Option<String>, pub split: bool, pub split_ratio: f64, pub sidebar_open: bool, pub detail_open: bool, pub active_library_id: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot { pub libraries: Vec<LibraryRoot>, pub documents: Vec<DocumentSummary>, pub tree: Vec<TreeNode>, pub groups: Vec<VirtualGroup>, pub materials: Vec<MaterialClip>, pub settings: UserSettings, pub session: AppSession }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentContent { pub summary: DocumentSummary, pub content: String, pub content_hash: String, pub encoding: String, pub newline: String, pub editable: bool, pub chapters: Vec<ChapterNode>, pub annotations: Vec<Annotation>, pub reading_progress: Option<ReadingProgress> }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteDocumentInput { pub document_id: String, pub content: String, pub expected_hash: String }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCreateInput { pub library_id: String, pub parent_path: String, pub name: String, pub kind: String }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct FileRenameInput { pub library_id: String, pub relative_path: String, pub new_name: String }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct FileMoveInput { pub source_library_id: String, pub relative_path: String, pub target_library_id: String, pub target_parent_path: String }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct FileTargetInput { pub library_id: String, pub relative_path: String }

#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct DocumentMetaInput { pub document_id: String, pub purpose: String, pub progress: String, pub length_kind: String, pub favorite: bool, pub gender: Option<String>, pub genre: Option<String>, pub subgenre: Option<String> }

#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct ChapterInput { pub document_id: String, pub title: String, pub offset: i64 }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct ChapterUpdateInput { pub id: String, pub document_id: String, pub title: String, pub offset: i64 }

#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct AnnotationInput { pub id: Option<String>, pub document_id: String, pub start_offset: i64, pub end_offset: i64, pub quote: String, pub prefix: String, pub suffix: String, pub note: String, pub marker: String }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct MaterialInput { pub document_id: String, pub quote: String, pub note: String, pub group_name: String, pub source_title: String, pub start_offset: i64 }

#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct SearchQuery { pub text: String, pub tag: Option<String>, pub library_id: Option<String>, pub length_kind: Option<String>, pub purpose: Option<String>, pub progress: Option<String>, pub format: Option<String> }
#[derive(Debug, Serialize)] #[serde(rename_all = "camelCase")]
pub struct SearchResult { pub document: DocumentSummary, pub snippet: String }

#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "camelCase")]
pub struct HistoryEntry { pub id: String, pub document_id: String, pub created_at: i64, pub word_count: i64, pub preview: String }
