use crate::{chapters, epub, models::*, AppState};
use encoding_rs::GBK;
use sha2::{Digest,Sha256};
use std::{fs::{self,File},io::Write,path::{Path,PathBuf},sync::Arc};
use uuid::Uuid;

pub struct DecodedText { pub text:String,pub encoding:String,pub newline:String }

/// 轻量预览：只取正文前几段（每段截断），供手机端卡片列表展示，避免整篇读取。
pub fn preview(state:&Arc<AppState>,id:&str,paragraphs:usize,chars_per_para:usize)->Result<Vec<String>,AppError>{
    let stored=state.db.stored_document(id)?;
    let path=safe_path(&stored.root_path,&stored.summary.relative_path)?;
    let bytes=fs::read(&path)?;
    let text=if stored.summary.format=="epub"{
        let parsed=epub::parse(&path,id)?;parsed.text
    }else{
        decode_text(&bytes)?.text
    };
    Ok(text.split('\n').map(|line|line.trim()).filter(|line|!line.is_empty()).take(paragraphs).map(|line|line.chars().take(chars_per_para).collect::<String>()).collect())
}

pub fn read(state:&Arc<AppState>,id:&str)->Result<DocumentContent,AppError>{
    let stored=state.db.stored_document(id)?;let path=safe_path(&stored.root_path,&stored.summary.relative_path)?;
    let bytes=fs::read(&path)?;
    let current_hash=hash_bytes(&bytes);
    // Indexing runs only on first read or when the file changed on disk; otherwise the cached
    // chapters / word count in the DB are reused (chapters are re-read below regardless).
    let changed=stored.content_hash.is_empty()||current_hash!=stored.content_hash;
    let mut detected:Vec<ChapterNode>=Vec::new();
    let (content,encoding,newline)=if stored.summary.format=="epub"{
        let p=epub::parse(&path,id)?;detected=p.chapters;(p.text,"epub".into(),"\n".into())
    }else{
        let decoded=decode_text(&bytes)?;(decoded.text,decoded.encoding,decoded.newline)
    };
    let mut words=stored.summary.word_count;
    if changed{
        if matches!(stored.summary.format.as_str(),"txt"|"md"){detected=chapters::detect(id,&content);}
        state.db.replace_auto_chapters(id,&detected)?;
        words=if matches!(stored.summary.format.as_str(),"txt"|"md"){chapters::count_words(&content)}else{0};
        state.db.update_after_read(id,&current_hash,&encoding,&newline,words)?;
    }
    let mut summary=stored.summary;
    if summary.word_count==0{summary.word_count=words;}
    if summary.length_kind=="auto"{summary.length_kind=if words>=80_000{"长篇"}else{"短篇"}.into();}
    let editable=matches!(path.extension().and_then(|s|s.to_str()).map(str::to_lowercase).as_deref(),Some("txt")|Some("md"));
    Ok(DocumentContent{summary,absolute_path:path.to_string_lossy().into_owned(),content,content_hash:current_hash,encoding,newline,editable,chapters:state.db.chapters(id)?,annotations:state.db.annotations(id)?,reading_progress:state.db.progress(id)?})
}

pub fn write(state:&Arc<AppState>,input:WriteDocumentInput)->Result<DocumentContent,AppError>{write_inner(state,input,false)}
pub fn force_write(state:&Arc<AppState>,input:WriteDocumentInput)->Result<DocumentContent,AppError>{write_inner(state,input,true)}

// 排版整理：去掉段落之间的空行分隔（每段一行，连续排列），其余内容原样保留。
pub fn tidy_text(text:&str)->String{
    let mut out:String=String::new();
    for line in text.lines(){
        if line.trim().is_empty(){continue;}
        if !out.is_empty(){out.push('\n');}
        out.push_str(line);
    }
    out
}

// 复用写管线（冲突检测 + 历史归档 + 换行归一），内容整理后写回磁盘。
pub fn tidy(state:&Arc<AppState>,id:&str)->Result<DocumentContent,AppError>{
    let stored=state.db.stored_document(id)?;
    if !matches!(stored.summary.format.as_str(),"txt"|"md"){return Err(AppError::Message("仅 TXT / 正文.md 支持排版整理".into()));}
    let path=safe_path(&stored.root_path,&stored.summary.relative_path)?;
    let bytes=fs::read(&path)?;
    let current_hash=hash_bytes(&bytes);
    let decoded=decode_text(&bytes)?;
    let cleaned=tidy_text(&decoded.text);
    if cleaned==decoded.text{return read(state,id);}
    write_inner(state,WriteDocumentInput{document_id:id.into(),content:cleaned,expected_hash:current_hash},false)
}
pub fn save_as(state:&Arc<AppState>,document_id:&str,content:&str,target_path:&str)->Result<(),AppError>{let _=state.db.stored_document(document_id)?;let path=PathBuf::from(target_path);if path.extension().and_then(|s|s.to_str()).map_or(true,|x|!x.eq_ignore_ascii_case("txt")){return Err(AppError::Message("另存为目标必须是 TXT 文件".into()));}if path.exists(){return Err(AppError::AlreadyExists);}atomic_write(&path,content.as_bytes())}
fn write_inner(state:&Arc<AppState>,input:WriteDocumentInput,force:bool)->Result<DocumentContent,AppError>{
    let stored=state.db.stored_document(&input.document_id)?;if !matches!(stored.summary.format.as_str(),"txt"|"md"){return Err(AppError::Message("EPUB 为只读格式".into()));}let path=safe_path(&stored.root_path,&stored.summary.relative_path)?;let old=fs::read(&path)?;let current_hash=hash_bytes(&old);if !force&&!input.expected_hash.is_empty()&&current_hash!=input.expected_hash{return Err(AppError::WriteConflict);}
    archive(state,&stored,&old)?;let normalized=if stored.newline=="\r\n"{input.content.replace("\r\n","\n").replace('\n',"\r\n")}else{input.content.clone()};atomic_write(&path,normalized.as_bytes())?;let hash=hash_bytes(normalized.as_bytes());let modified=fs::metadata(&path)?.modified().ok().and_then(|t|t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_secs()as i64).unwrap_or(0);state.db.update_after_write(&input.document_id,&hash,chapters::count_words(&input.content),modified)?;read(state,&input.document_id)
}
pub fn restore_history(state:&Arc<AppState>,history_id:&str,document_id:&str)->Result<DocumentContent,AppError>{let bytes=fs::read(state.db.history_path(history_id,document_id)?)?;let stored=state.db.stored_document(document_id)?;let path=safe_path(&stored.root_path,&stored.summary.relative_path)?;let current=fs::read(&path)?;archive(state,&stored,&current)?;atomic_write(&path,&bytes)?;read(state,document_id)}
fn archive(state:&Arc<AppState>,stored:&crate::database::StoredDocument,bytes:&[u8])->Result<(),AppError>{let dir=state.data_dir.join("history").join(&stored.summary.id);fs::create_dir_all(&dir)?;let id=Uuid::new_v4().to_string();let path=dir.join(format!("{}.txt",id));atomic_write(&path,bytes)?;let decoded=decode_text(bytes).map(|d|d.text).unwrap_or_default();let preview: String=decoded.chars().take(120).collect();state.db.add_history(&id,&stored.summary.id,&path,chapters::count_words(&decoded),&preview)?;for old in state.db.prune_history(&stored.summary.id)?{let _=fs::remove_file(old);}Ok(())}
pub fn decode_text(bytes:&[u8])->Result<DecodedText,AppError>{let (text,enc)=if bytes.starts_with(&[0xef,0xbb,0xbf]){(std::str::from_utf8(&bytes[3..]).map_err(|_|AppError::Message("TXT 编码无法识别".into()))?.to_string(),"utf-8-bom")}else if let Ok(s)=std::str::from_utf8(bytes){(s.to_string(),"utf-8")}else{let(s,_,had)=GBK.decode(bytes);if had{return Err(AppError::Message("TXT 编码无法识别，请转换为 UTF-8 或 GBK".into()));}(s.into_owned(),"gb18030")};let newline=if text.contains("\r\n"){"\r\n"}else{"\n"};Ok(DecodedText{text:text.replace("\r\n","\n"),encoding:enc.into(),newline:newline.into()})}
pub fn safe_path(root:&Path,relative:&str)->Result<PathBuf,AppError>{if relative.is_empty()||Path::new(relative).is_absolute(){return Err(AppError::PathOutsideLibrary);}let root=root.canonicalize().map_err(|_|AppError::NotFound)?;let candidate=root.join(relative);if candidate.exists(){let canonical=candidate.canonicalize().map_err(|_|AppError::NotFound)?;if !canonical.starts_with(&root){return Err(AppError::PathOutsideLibrary);}Ok(canonical)}else{let parent=candidate.parent().ok_or(AppError::PathOutsideLibrary)?.canonicalize().map_err(|_|AppError::NotFound)?;if !parent.starts_with(&root){return Err(AppError::PathOutsideLibrary);}Ok(candidate)}}
pub fn hash_bytes(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn atomic_write(path:&Path,bytes:&[u8])->Result<(),AppError>{let parent=path.parent().ok_or(AppError::PathOutsideLibrary)?;let temp=parent.join(format!(".novalyte-{}.tmp",Uuid::new_v4()));let mut file=File::create(&temp)?;file.write_all(bytes)?;file.sync_all()?;fs::rename(&temp,path)?;Ok(())}

#[cfg(test)]
mod tests {
    use super::tidy_text;

    #[test]
    fn tidy_removes_blank_lines(){
        // 段落间的空行分隔去掉，每段一行连续排列
        assert_eq!(tidy_text("第一段。\n\n第二段。\n\n\n第三段。"),"第一段。\n第二段。\n第三段。");
        // 已经无空行的文本保持不变（仅统一换行）
        assert_eq!(tidy_text("第一段。\n第二段。"),"第一段。\n第二段。");
        // CRLF 与全空格行
        assert_eq!(tidy_text("第一段。\r\n  \r\n第二段。\r\n"),"第一段。\n第二段。");
        // 空文件
        assert_eq!(tidy_text(""),"");
        // 行内缩进保留，不做其他排版
        assert_eq!(tidy_text("　第一段。\n\n第二段。"),"　第一段。\n第二段。");
    }
}
