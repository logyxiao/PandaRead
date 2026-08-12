use crate::models::*;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::{HashMap,HashSet}, fs, path::{Path,PathBuf}, sync::Arc};
use uuid::Uuid;

#[derive(Clone)]
pub struct Database { conn: Arc<Mutex<Connection>> }

#[derive(Debug, Clone)]
pub struct StoredDocument { pub summary: DocumentSummary, pub root_path: PathBuf, pub content_hash: String, pub newline: String }

pub struct ScanEntry { pub library_id:String,pub relative_path:String,pub title:String,pub format:String,pub word_count:i64,pub modified_at:i64,pub hash:String,pub encoding:String,pub newline:String,pub taxonomy:[String;3] }

impl Database {
    pub fn open(path: PathBuf) -> Result<Self, AppError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(r#"
          CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY);
          CREATE TABLE IF NOT EXISTS libraries(
            id TEXT PRIMARY KEY, root_path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, created_at INTEGER NOT NULL
          );
          CREATE TABLE IF NOT EXISTS documents(
            id TEXT PRIMARY KEY, library_id TEXT NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
            relative_path TEXT NOT NULL, title TEXT NOT NULL, format TEXT NOT NULL,
            word_count INTEGER NOT NULL DEFAULT 0, modified_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT NOT NULL DEFAULT '', encoding TEXT NOT NULL DEFAULT 'utf-8', newline TEXT NOT NULL DEFAULT '\n',
            gender TEXT NOT NULL DEFAULT '未分类', genre TEXT NOT NULL DEFAULT '未分类', subgenre TEXT NOT NULL DEFAULT '未分类',
            length_kind TEXT NOT NULL DEFAULT 'auto', purpose TEXT NOT NULL DEFAULT '原创', progress TEXT NOT NULL DEFAULT '构思中',
            favorite INTEGER NOT NULL DEFAULT 0, missing INTEGER NOT NULL DEFAULT 0,
            UNIQUE(library_id, relative_path)
          );
          CREATE TABLE IF NOT EXISTS chapters(
            id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            title TEXT NOT NULL, offset INTEGER NOT NULL, kind TEXT NOT NULL, level INTEGER NOT NULL DEFAULT 1
          );
          CREATE TABLE IF NOT EXISTS reading_progress(
            document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
            chapter_id TEXT, char_offset INTEGER NOT NULL, scroll_ratio REAL NOT NULL, updated_at INTEGER NOT NULL
          );
          CREATE TABLE IF NOT EXISTS document_tags(
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            tag TEXT NOT NULL, PRIMARY KEY(document_id, tag)
          );
          CREATE TABLE IF NOT EXISTS annotations(
            id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            start_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL, quote TEXT NOT NULL,
            prefix TEXT NOT NULL, suffix TEXT NOT NULL, note TEXT NOT NULL, marker TEXT NOT NULL,
            orphaned INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL
          );
          CREATE TABLE IF NOT EXISTS materials(
            id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            quote TEXT NOT NULL, note TEXT NOT NULL, group_name TEXT NOT NULL, source_title TEXT NOT NULL,
            start_offset INTEGER NOT NULL, created_at INTEGER NOT NULL
          );
          CREATE TABLE IF NOT EXISTS groups(id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS group_documents(
            group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            PRIMARY KEY(group_id, document_id)
          );
          CREATE TABLE IF NOT EXISTS history(
            id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            file_path TEXT NOT NULL, created_at INTEGER NOT NULL, word_count INTEGER NOT NULL, preview TEXT NOT NULL
          );
          CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
          INSERT OR IGNORE INTO schema_migrations(version) VALUES(1);
        "#)?;
        // Migration v2: drop the dead FTS index (content was never indexed) and reclaim the space
        // it accumulated as deleted-row garbage over repeated rescans.
        let migrated: bool = conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM schema_migrations WHERE version>=2)", [], |r| r.get(0))?;
        if migrated {
            conn.execute_batch("DROP TABLE IF EXISTS documents_fts;")?;
            let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            if free > 20_000 { conn.execute_batch("VACUUM;")?; }
            conn.execute_batch("INSERT OR IGNORE INTO schema_migrations(version) VALUES(2);")?;
        }
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn snapshot(&self) -> Result<AppSnapshot, AppError> {
        Ok(AppSnapshot {
            libraries: self.libraries()?, documents: self.documents()?, tree: self.tree()?,
            groups: self.groups()?, materials: self.materials()?, settings: self.settings()?, session: self.session()?,
        })
    }

    pub fn libraries(&self) -> Result<Vec<LibraryRoot>, AppError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT l.id,l.name,count(d.id) FROM libraries l LEFT JOIN documents d ON d.library_id=l.id AND d.missing=0 GROUP BY l.id ORDER BY l.created_at")?;
        let rows = stmt.query_map([], |r| Ok(LibraryRoot { id:r.get(0)?, name:r.get(1)?, document_count:r.get(2)? }))?.collect::<Result<_,_>>()?;
        Ok(rows)
    }

    pub fn library_paths(&self) -> Result<Vec<(String, PathBuf, String)>, AppError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id,root_path,name FROM libraries ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, PathBuf::from(r.get::<_,String>(1)?), r.get(2)?)))?.collect::<Result<_,_>>()?;
        Ok(rows)
    }

    pub fn add_library(&self, root: &Path, name: &str) -> Result<String, AppError> {
        let root = root.canonicalize().map_err(|_| AppError::NotFound)?;
        if !root.is_dir() { return Err(AppError::NotFound); }
        let text = root.to_string_lossy();
        let conn = self.conn.lock();
        if let Some(id) = conn.query_row("SELECT id FROM libraries WHERE root_path=?1", [text.as_ref()], |r| r.get(0)).optional()? { return Ok(id); }
        let id = Uuid::new_v4().to_string();
        conn.execute("INSERT INTO libraries(id,root_path,name,created_at) VALUES(?1,?2,?3,?4)", params![id,text.as_ref(),name,now()])?;
        Ok(id)
    }

    pub fn remove_library(&self, id: &str) -> Result<(), AppError> {
        // Cascade-delete everything that belongs to this library; files on disk are untouched.
        let conn=self.conn.lock();let tx=conn.unchecked_transaction()?;
        for table in ["chapters","annotations","reading_progress","history","group_documents","materials","document_tags"]{
            tx.execute(&format!("DELETE FROM {} WHERE document_id IN (SELECT id FROM documents WHERE library_id=?1)",table),[id])?;
        }
        tx.execute("DELETE FROM documents WHERE library_id=?1",[id])?;
        tx.execute("DELETE FROM libraries WHERE id=?1",[id])?;
        tx.commit()?;Ok(())
    }

    pub fn relocate_documents(&self, source_library_id:&str, source_path:&str, target_library_id:&str, target_path:&str)->Result<(),AppError>{
        let conn=self.conn.lock();
        let mut stmt=conn.prepare("SELECT id,relative_path FROM documents WHERE library_id=?1 AND (relative_path=?2 OR relative_path LIKE ?3)")?;
        let prefix=format!("{}/%",source_path);
        let rows=stmt.query_map(params![source_library_id,source_path,prefix],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        drop(stmt);
        for(id,old)in rows{let suffix=old.strip_prefix(source_path).unwrap_or("");let next=format!("{}{}",target_path,suffix);conn.execute("UPDATE documents SET library_id=?2,relative_path=?3,missing=0 WHERE id=?1",params![id,target_library_id,next])?;}
        Ok(())
    }

    pub fn upsert_documents(&self, entries:&[ScanEntry]) -> Result<(),AppError> {
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        for e in entries {
            let existing: Option<String> = tx.query_row("SELECT id FROM documents WHERE library_id=?1 AND relative_path=?2",
                params![e.library_id,e.relative_path], |r| r.get(0)).optional()?;
            let id = existing.unwrap_or_else(||Uuid::new_v4().to_string());
            // Lazy indexing: digest fields (hash/encoding/word count) are filled in on first read
            // and only updated by read/write — never overwritten by a directory rescan.
            tx.execute(r#"INSERT INTO documents(id,library_id,relative_path,title,format,word_count,modified_at,content_hash,encoding,newline,gender,genre,subgenre,length_kind,missing)
              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,0)
              ON CONFLICT(library_id,relative_path) DO UPDATE SET title=excluded.title,format=excluded.format,modified_at=excluded.modified_at,missing=0"#,
              params![id,e.library_id,e.relative_path,e.title,e.format,e.word_count,e.modified_at,e.hash,e.encoding,e.newline,e.taxonomy[0],e.taxonomy[1],e.taxonomy[2],"auto"])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_after_read(&self, id:&str, hash:&str, encoding:&str, newline:&str, words:i64) -> Result<(),AppError>{
        self.conn.lock().execute("UPDATE documents SET content_hash=?2,encoding=?3,newline=?4,word_count=?5,length_kind=CASE WHEN length_kind='auto' THEN ?6 ELSE length_kind END WHERE id=?1",params![id,hash,encoding,newline,words,if words>=80_000{"长篇"}else{"短篇"}])?;Ok(())
    }

    pub fn update_missing_flags(&self, library_id:&str, seen:&[String]) -> Result<(),AppError>{
        let conn=self.conn.lock();let tx=conn.unchecked_transaction()?;
        tx.execute("UPDATE documents SET missing=1 WHERE library_id=?1",[library_id])?;
        for chunk in seen.chunks(500){
            let placeholders=vec!["?";chunk.len()].join(",");
            tx.execute(&format!("UPDATE documents SET missing=0 WHERE library_id=?1 AND relative_path IN ({})",placeholders),rusqlite::params_from_iter(std::iter::once(library_id).chain(chunk.iter().map(|s|s.as_str()))))?;
        }
        tx.commit()?;Ok(())
    }

    pub fn documents(&self) -> Result<Vec<DocumentSummary>,AppError> {
        let conn=self.conn.lock(); let mut stmt=conn.prepare("SELECT id,library_id,relative_path,title,format,word_count,modified_at,gender,genre,subgenre,length_kind,purpose,progress,favorite,missing,(SELECT COALESCE(json_group_array(tag),'[]') FROM document_tags WHERE document_id=documents.id) FROM documents ORDER BY relative_path COLLATE NOCASE")?;
        let rows = stmt.query_map([], row_document)?.collect::<Result<_,_>>()?;
        Ok(rows)
    }

    pub fn update_tags(&self, document_id:&str, tags:&[String])->Result<DocumentSummary,AppError>{
        let conn=self.conn.lock();let tx=conn.unchecked_transaction()?;
        tx.execute("DELETE FROM document_tags WHERE document_id=?1",[document_id])?;
        let mut seen=HashSet::new();
        for tag in tags{
            let tag=tag.trim().trim_start_matches('#').trim();
            let key=tag.to_lowercase();
            if !tag.is_empty()&&seen.insert(key){tx.execute("INSERT INTO document_tags(document_id,tag) VALUES(?1,?2)",params![document_id,tag])?;}
        }
        tx.commit()?;
        drop(conn);
        Ok(self.stored_document(document_id)?.summary)
    }

    pub fn stored_document(&self,id:&str)->Result<StoredDocument,AppError>{
        let conn=self.conn.lock();
        conn.query_row(r#"SELECT d.id,d.library_id,d.relative_path,d.title,d.format,d.word_count,d.modified_at,d.gender,d.genre,d.subgenre,d.length_kind,d.purpose,d.progress,d.favorite,d.missing,l.root_path,d.content_hash,d.encoding,d.newline,(SELECT COALESCE(json_group_array(tag),'[]') FROM document_tags WHERE document_id=d.id)
          FROM documents d JOIN libraries l ON l.id=d.library_id WHERE d.id=?1"#, [id], |r| Ok(StoredDocument{
            summary: DocumentSummary{id:r.get(0)?,library_id:r.get(1)?,relative_path:r.get(2)?,title:r.get(3)?,format:r.get(4)?,word_count:r.get(5)?,modified_at:r.get(6)?,gender:r.get(7)?,genre:r.get(8)?,subgenre:r.get(9)?,length_kind:r.get(10)?,purpose:r.get(11)?,progress:r.get(12)?,favorite:r.get::<_,i64>(13)?!=0,missing:r.get::<_,i64>(14)?!=0,tags:serde_json::from_str(&r.get::<_,String>(19)?).unwrap_or_default()},
            root_path:PathBuf::from(r.get::<_,String>(15)?),content_hash:r.get(16)?,newline:r.get(18)?
        })).optional()?.ok_or(AppError::NotFound)
    }

    pub fn update_after_write(&self,id:&str,hash:&str,words:i64,modified:i64)->Result<(),AppError>{
        let conn=self.conn.lock(); conn.execute("UPDATE documents SET content_hash=?2,encoding='utf-8',word_count=?3,modified_at=?4,missing=0 WHERE id=?1",params![id,hash,words,modified])?; Ok(())
    }

    pub fn replace_auto_chapters(&self, document_id:&str, chapters:&[ChapterNode])->Result<(),AppError>{
        let conn=self.conn.lock(); let tx=conn.unchecked_transaction()?; tx.execute("DELETE FROM chapters WHERE document_id=?1 AND kind='auto'",[document_id])?;
        for c in chapters { tx.execute("INSERT INTO chapters(id,document_id,title,offset,kind,level) VALUES(?1,?2,?3,?4,?5,?6)",params![c.id,c.document_id,c.title,c.offset,c.kind,c.level])?; } tx.commit()?; Ok(())
    }
    pub fn chapters(&self,id:&str)->Result<Vec<ChapterNode>,AppError>{ let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,document_id,title,offset,kind,level FROM chapters WHERE document_id=?1 ORDER BY offset,kind DESC")?;let rows=s.query_map([id],|r|Ok(ChapterNode{id:r.get(0)?,document_id:r.get(1)?,title:r.get(2)?,offset:r.get(3)?,kind:r.get(4)?,level:r.get(5)?}))?.collect::<Result<_,_>>()?;Ok(rows) }
    pub fn create_chapter(&self,input:ChapterInput)->Result<Vec<ChapterNode>,AppError>{self.conn.lock().execute("INSERT INTO chapters(id,document_id,title,offset,kind,level) VALUES(?1,?2,?3,?4,'manual',1)",params![Uuid::new_v4().to_string(),input.document_id,input.title,input.offset])?;self.chapters(&input.document_id)}
    pub fn update_chapter(&self,input:ChapterUpdateInput)->Result<Vec<ChapterNode>,AppError>{self.conn.lock().execute("UPDATE chapters SET title=?3,offset=?4 WHERE id=?1 AND document_id=?2 AND kind='manual'",params![input.id,input.document_id,input.title,input.offset])?;self.chapters(&input.document_id)}
    pub fn delete_chapter(&self,id:&str,doc:&str)->Result<Vec<ChapterNode>,AppError>{self.conn.lock().execute("DELETE FROM chapters WHERE id=?1 AND document_id=?2 AND kind='manual'",params![id,doc])?;self.chapters(doc)}

    pub fn annotations(&self,id:&str)->Result<Vec<Annotation>,AppError>{let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,document_id,start_offset,end_offset,quote,prefix,suffix,note,marker,orphaned,created_at FROM annotations WHERE document_id=?1 ORDER BY start_offset")?;let rows=s.query_map([id],|r|Ok(Annotation{id:r.get(0)?,document_id:r.get(1)?,start_offset:r.get(2)?,end_offset:r.get(3)?,quote:r.get(4)?,prefix:r.get(5)?,suffix:r.get(6)?,note:r.get(7)?,marker:r.get(8)?,orphaned:r.get::<_,i64>(9)?!=0,created_at:r.get(10)?}))?.collect::<Result<_,_>>()?;Ok(rows)}
    pub fn save_annotation(&self,input:AnnotationInput)->Result<Vec<Annotation>,AppError>{let id=input.id.unwrap_or_else(||Uuid::new_v4().to_string());self.conn.lock().execute(r#"INSERT INTO annotations(id,document_id,start_offset,end_offset,quote,prefix,suffix,note,marker,orphaned,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10) ON CONFLICT(id) DO UPDATE SET note=excluded.note,marker=excluded.marker,start_offset=excluded.start_offset,end_offset=excluded.end_offset,orphaned=0"#,params![id,input.document_id,input.start_offset,input.end_offset,input.quote,input.prefix,input.suffix,input.note,input.marker,now()])?;self.annotations(&input.document_id)}
    pub fn delete_annotation(&self,id:&str,doc:&str)->Result<Vec<Annotation>,AppError>{self.conn.lock().execute("DELETE FROM annotations WHERE id=?1 AND document_id=?2",params![id,doc])?;self.annotations(doc)}

    pub fn save_material(&self,input:MaterialInput)->Result<Vec<MaterialClip>,AppError>{self.conn.lock().execute("INSERT INTO materials(id,document_id,quote,note,group_name,source_title,start_offset,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![Uuid::new_v4().to_string(),input.document_id,input.quote,input.note,input.group_name,input.source_title,input.start_offset,now()])?;self.materials()}
    pub fn materials(&self)->Result<Vec<MaterialClip>,AppError>{let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,document_id,quote,note,group_name,source_title,start_offset,created_at FROM materials ORDER BY created_at DESC")?;let rows=s.query_map([],|r|Ok(MaterialClip{id:r.get(0)?,document_id:r.get(1)?,quote:r.get(2)?,note:r.get(3)?,group_name:r.get(4)?,source_title:r.get(5)?,start_offset:r.get(6)?,created_at:r.get(7)?}))?.collect::<Result<_,_>>()?;Ok(rows)}

    pub fn create_group(&self,name:&str)->Result<(),AppError>{let name=name.trim();if name.is_empty(){return Err(AppError::Message("分组名称不能为空".into()));}self.conn.lock().execute("INSERT INTO groups(id,name,created_at) VALUES(?1,?2,?3)",params![Uuid::new_v4().to_string(),name,now()]).map_err(|e|if matches!(e,rusqlite::Error::SqliteFailure(_, _)){AppError::AlreadyExists}else{e.into()})?;Ok(())}
    pub fn toggle_group_document(&self,g:&str,d:&str)->Result<(),AppError>{let conn=self.conn.lock();let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM group_documents WHERE group_id=?1 AND document_id=?2)",params![g,d],|r|r.get(0))?;if exists{conn.execute("DELETE FROM group_documents WHERE group_id=?1 AND document_id=?2",params![g,d])?;}else{conn.execute("INSERT INTO group_documents(group_id,document_id) VALUES(?1,?2)",params![g,d])?;}Ok(())}
    pub fn groups(&self)->Result<Vec<VirtualGroup>,AppError>{let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,name FROM groups ORDER BY created_at")?;let base=s.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;let mut out=Vec::new();for(id,name)in base{let mut ds=conn.prepare("SELECT document_id FROM group_documents WHERE group_id=?1")?;let ids=ds.query_map([&id],|r|r.get(0))?.collect::<Result<_,_>>()?;out.push(VirtualGroup{id,name,document_ids:ids});}Ok(out)}

    pub fn update_document_meta(&self,i:DocumentMetaInput)->Result<(),AppError>{self.conn.lock().execute("UPDATE documents SET purpose=?2,progress=?3,length_kind=?4,favorite=?5,gender=COALESCE(?6,gender),genre=COALESCE(?7,genre),subgenre=COALESCE(?8,subgenre) WHERE id=?1",params![i.document_id,i.purpose,i.progress,i.length_kind,i.favorite as i64,i.gender,i.genre,i.subgenre])?;Ok(())}
    pub fn save_progress(&self,p:ReadingProgress)->Result<(),AppError>{self.conn.lock().execute(r#"INSERT INTO reading_progress(document_id,chapter_id,char_offset,scroll_ratio,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(document_id) DO UPDATE SET chapter_id=excluded.chapter_id,char_offset=excluded.char_offset,scroll_ratio=excluded.scroll_ratio,updated_at=excluded.updated_at"#,params![p.document_id,p.chapter_id,p.char_offset,p.scroll_ratio,now()])?;Ok(())}
    pub fn progress(&self,id:&str)->Result<Option<ReadingProgress>,AppError>{Ok(self.conn.lock().query_row("SELECT document_id,chapter_id,char_offset,scroll_ratio FROM reading_progress WHERE document_id=?1",[id],|r|Ok(ReadingProgress{document_id:r.get(0)?,chapter_id:r.get(1)?,char_offset:r.get(2)?,scroll_ratio:r.get(3)?})).optional()?)}

    pub fn save_settings(&self,s:&UserSettings)->Result<(),AppError>{let value=serde_json::to_string(s).map_err(|e|AppError::Message(e.to_string()))?;self.conn.lock().execute("INSERT INTO settings(key,value) VALUES('user',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[value])?;Ok(())}
    pub fn settings(&self)->Result<UserSettings,AppError>{let v:Option<String>=self.conn.lock().query_row("SELECT value FROM settings WHERE key='user'",[],|r|r.get(0)).optional()?;Ok(v.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default())}
    pub fn save_session(&self,s:&AppSession)->Result<(),AppError>{let value=serde_json::to_string(s).map_err(|e|AppError::Message(e.to_string()))?;self.conn.lock().execute("INSERT INTO settings(key,value) VALUES('session',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[value])?;Ok(())}
    pub fn session(&self)->Result<AppSession,AppError>{let v:Option<String>=self.conn.lock().query_row("SELECT value FROM settings WHERE key='session'",[],|r|r.get(0)).optional()?;Ok(v.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or(AppSession{split_ratio:0.5,sidebar_open:true,detail_open:true,..Default::default()}))}

    pub fn search(&self,q:SearchQuery)->Result<Vec<SearchResult>,AppError>{
        let docs=self.documents()?;let needle=q.text.trim().to_lowercase();
        let tag=q.tag.as_deref().map(str::trim).filter(|value|!value.is_empty()).map(str::to_lowercase);
        let matched:HashSet<String>=if needle.is_empty(){HashSet::new()}else{docs.iter().filter(|d|d.title.to_lowercase().contains(&needle)||d.relative_path.to_lowercase().contains(&needle)||d.tags.iter().any(|t|t.to_lowercase().contains(&needle))).map(|d|d.id.clone()).collect()};
        Ok(docs.into_iter().filter(|d|!d.missing&&(needle.is_empty()||matched.contains(&d.id))&&tag.as_ref().map_or(true,|value|d.tags.iter().any(|item|item.to_lowercase()==*value))&&q.library_id.as_ref().map_or(true,|v|&d.library_id==v)&&q.length_kind.as_ref().map_or(true,|v|&d.length_kind==v)&&q.purpose.as_ref().map_or(true,|v|&d.purpose==v)&&q.progress.as_ref().map_or(true,|v|&d.progress==v)&&q.format.as_ref().map_or(true,|v|&d.format==v)).map(|d|SearchResult{snippet:d.relative_path.clone(),document:d}).collect())
    }

    pub fn add_history(&self,id:&str,doc:&str,path:&Path,words:i64,preview:&str)->Result<(),AppError>{self.conn.lock().execute("INSERT INTO history(id,document_id,file_path,created_at,word_count,preview) VALUES(?1,?2,?3,?4,?5,?6)",params![id,doc,path.to_string_lossy(),now(),words,preview])?;Ok(())}
    pub fn history(&self,doc:&str)->Result<Vec<HistoryEntry>,AppError>{let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,document_id,created_at,word_count,preview FROM history WHERE document_id=?1 ORDER BY created_at DESC")?;let rows=s.query_map([doc],|r|Ok(HistoryEntry{id:r.get(0)?,document_id:r.get(1)?,created_at:r.get(2)?,word_count:r.get(3)?,preview:r.get(4)?}))?.collect::<Result<_,_>>()?;Ok(rows)}
    pub fn history_path(&self,id:&str,doc:&str)->Result<PathBuf,AppError>{self.conn.lock().query_row("SELECT file_path FROM history WHERE id=?1 AND document_id=?2",params![id,doc],|r|r.get::<_,String>(0)).optional()?.map(PathBuf::from).ok_or(AppError::NotFound)}
    pub fn prune_history(&self,doc:&str)->Result<Vec<PathBuf>,AppError>{let cutoff=now()-30*86400;let conn=self.conn.lock();let mut s=conn.prepare("SELECT id,file_path FROM history WHERE document_id=?1 AND (created_at<?2 OR id NOT IN(SELECT id FROM history WHERE document_id=?1 ORDER BY created_at DESC LIMIT 20))")?;let old=s.query_map(params![doc,cutoff],|r|Ok((r.get::<_,String>(0)?,PathBuf::from(r.get::<_,String>(1)?))))?.collect::<Result<Vec<_>,_>>()?;for(id,_)in&old{conn.execute("DELETE FROM history WHERE id=?1",[id])?;}Ok(old.into_iter().map(|x|x.1).collect())}

    fn tree(&self)->Result<Vec<TreeNode>,AppError>{
        let docs=self.documents()?;
        let doc_by_key:HashMap<(String,String),String>=docs.iter().map(|d|((d.library_id.clone(),d.relative_path.clone()),d.id.clone())).collect();
        let libs=self.library_paths()?;
        let mut roots=Vec::new();
        for(lib_id,root_path,lib_name)in libs{
            let mut root=TreeNode{name:lib_name,relative_path:String::new(),kind:"library".into(),library_id:lib_id.clone(),document_id:None,count:0,children:Vec::new()};
            build_tree(&mut root,&root_path,&root_path,&lib_id,&doc_by_key);
            roots.push(root);
        }
        Ok(roots)
    }
}

fn row_document(r:&rusqlite::Row<'_>)->rusqlite::Result<DocumentSummary>{Ok(DocumentSummary{id:r.get(0)?,library_id:r.get(1)?,relative_path:r.get(2)?,title:r.get(3)?,format:r.get(4)?,word_count:r.get(5)?,modified_at:r.get(6)?,gender:r.get(7)?,genre:r.get(8)?,subgenre:r.get(9)?,length_kind:r.get(10)?,purpose:r.get(11)?,progress:r.get(12)?,favorite:r.get::<_,i64>(13)?!=0,missing:r.get::<_,i64>(14)?!=0,tags:serde_json::from_str(&r.get::<_,String>(15)?).unwrap_or_default()})}
fn build_tree(node:&mut TreeNode,root:&Path,dir:&Path,library_id:&str,doc_by_key:&HashMap<(String,String),String>){
    let Ok(entries)=fs::read_dir(dir)else{return;};
    let mut entries:Vec<_>=entries.flatten().collect();
    // 文件夹排最上，再按名称排序
    entries.sort_by(|a,b|{
        let ak=a.file_type().map(|t|t.is_dir()).unwrap_or(false);
        let bk=b.file_type().map(|t|t.is_dir()).unwrap_or(false);
        bk.cmp(&ak).then_with(||a.file_name().to_string_lossy().to_lowercase().cmp(&b.file_name().to_string_lossy().to_lowercase()))
    });
    for entry in entries{
        let path=entry.path();
        let name=entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')||name=="__MACOSX"||name=="node_modules"{continue;}
        let ft=match entry.file_type(){Ok(t)=>t,Err(_)=>continue};
        let relative=path.strip_prefix(root).map(|p|p.to_string_lossy().replace('\\',"/")).unwrap_or_default();
        if ft.is_dir(){
            let mut child=TreeNode{name,relative_path:relative,kind:"folder".into(),library_id:library_id.into(),document_id:None,count:0,children:Vec::new()};
            build_tree(&mut child,root,&path,library_id,doc_by_key);
            node.children.push(child);
        }else if ft.is_file(){
            let ext=path.extension().and_then(|s|s.to_str()).unwrap_or("").to_lowercase();
            if ext!="txt"&&ext!="epub"{continue;}
            let id=doc_by_key.get(&(library_id.to_string(),relative.clone())).cloned();
            node.children.push(TreeNode{name,relative_path:relative,kind:"document".into(),library_id:library_id.into(),document_id:id,count:0,children:Vec::new()});
        }
    }
    let direct=node.children.iter().filter(|c|c.kind=="document").count();
    let sub:usize=node.children.iter().filter(|c|c.kind=="folder").map(|c|c.count as usize).sum();
    node.count=(direct+sub)as i64;
}
fn now()->i64{chrono::Utc::now().timestamp()}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(relative:&str, modified:i64)->ScanEntry{
        ScanEntry{library_id:"l1".into(),relative_path:relative.into(),title:relative.into(),format:"txt".into(),word_count:0,modified_at:modified,hash:String::new(),encoding:"utf-8".into(),newline:"\n".into(),taxonomy:["未分类".into(),"未分类".into(),"未分类".into()]}
    }

    #[test]
    fn lazy_scan_backfills_on_read(){
        let db=Database::open(":memory:".into()).unwrap();
        db.conn.lock().execute("INSERT INTO libraries(id,root_path,name,created_at) VALUES('l1','/tmp/x','t',1)",[]).unwrap();
        // import stores no content digest
        db.upsert_documents(&[entry("a.txt",5),entry("b.txt",6)]).unwrap();
        let docs=db.documents().unwrap();
        assert_eq!(docs.len(),2);
        assert!(docs.iter().all(|d|d.word_count==0&&d.length_kind=="auto"));
        let id=docs.iter().find(|d|d.relative_path=="a.txt").unwrap().id.clone();
        // first read backfills word count / hash / length_kind
        db.update_after_read(&id,"hash1","utf-8","\n",100_000).unwrap();
        let d=db.stored_document(&id).unwrap();
        assert_eq!(d.summary.word_count,100_000);
        assert_eq!(d.summary.length_kind,"长篇");
        assert_eq!(d.content_hash,"hash1");
        // manual length_kind survives later reads
        db.update_document_meta(DocumentMetaInput{document_id:id.clone(),purpose:"原创".into(),progress:"构思中".into(),length_kind:"短篇".into(),favorite:false,gender:None,genre:None,subgenre:None}).unwrap();
        db.update_after_read(&id,"hash2","utf-8","\n",100_000).unwrap();
        assert_eq!(db.stored_document(&id).unwrap().summary.length_kind,"短篇");
        // seen files are not missing, unseen ones are; re-upsert keeps rows stable
        db.update_missing_flags("l1",&["a.txt".into()]).unwrap();
        let docs=db.documents().unwrap();
        assert!(docs.iter().find(|x|x.relative_path=="a.txt").unwrap().missing==false);
        assert!(docs.iter().find(|x|x.relative_path=="b.txt").unwrap().missing==true);
        // a rescan (lazy entry, empty digest) must not wipe already-computed digests
        db.upsert_documents(&[entry("a.txt",5)]).unwrap();
        assert_eq!(db.documents().unwrap().len(),2);
        assert_eq!(db.stored_document(&id).unwrap().summary.word_count,100_000);
        assert_eq!(db.stored_document(&id).unwrap().content_hash,"hash2");
    }

    #[test]
    fn remove_library_cascades(){
        let db=Database::open(":memory:".into()).unwrap();
        db.conn.lock().execute("INSERT INTO libraries(id,root_path,name,created_at) VALUES('l1','/tmp/x','t',1)",[]).unwrap();
        db.upsert_documents(&[entry("a.txt",5)]).unwrap();
        let id=db.documents().unwrap()[0].id.clone();
        db.save_annotation(AnnotationInput{id:None,document_id:id.clone(),start_offset:0,end_offset:3,quote:"q".into(),prefix:String::new(),suffix:String::new(),note:String::new(),marker:"钩子".into()}).unwrap();
        db.add_history("h1",&id,std::path::Path::new("/tmp/x/a.txt"),100,"preview").unwrap();
        db.save_progress(ReadingProgress{document_id:id.clone(),chapter_id:None,char_offset:0,scroll_ratio:0.0}).unwrap();
        db.remove_library("l1").unwrap();
        assert!(db.documents().unwrap().is_empty());
        assert!(db.annotations(&id).unwrap().is_empty());
        assert!(db.history(&id).unwrap().is_empty());
        assert!(db.progress(&id).unwrap().is_none());
        assert!(db.libraries().unwrap().is_empty());
    }

    #[test]
    fn tree_includes_empty_folders_and_counts(){
        let db=Database::open(":memory:".into()).unwrap();
        let tmp=std::env::temp_dir().join(format!("novalyte-tree-test-{}",Uuid::new_v4()));
        fs::create_dir_all(tmp.join("a/empty")).unwrap();
        fs::create_dir_all(tmp.join("b")).unwrap();
        fs::write(tmp.join("a/x.txt"),"hi").unwrap();
        fs::write(tmp.join("b/y.txt"),"hi").unwrap();
        fs::write(tmp.join("b/ignore.txt"),"hi").unwrap();
        fs::write(tmp.join("b/.h.txt"),"hi").unwrap();
        let root=tmp.canonicalize().unwrap();
        db.conn.lock().execute("INSERT INTO libraries(id,root_path,name,created_at) VALUES('l1',?1,'t',1)",[root.to_string_lossy().to_string()]).unwrap();
        db.upsert_documents(&[entry("a/x.txt",1),entry("b/y.txt",1)]).unwrap();
        let tree=db.tree().unwrap();
        let lib=&tree[0];
        assert_eq!(lib.count,3);
        let a=lib.children.iter().find(|c|c.name=="a").unwrap();
        assert_eq!(a.count,1);
        assert!(a.children.iter().find(|c|c.name=="x.txt").unwrap().document_id.is_some());
        let empty=a.children.iter().find(|c|c.name=="empty").unwrap();
        assert_eq!(empty.kind,"folder");
        assert_eq!(empty.count,0);
        assert!(empty.children.is_empty());
        // 文件夹排在文件前面
        assert_eq!(a.children[0].name,"empty");
        assert_eq!(a.children[0].kind,"folder");
        let b=lib.children.iter().find(|c|c.name=="b").unwrap();
        assert_eq!(b.count,2);
        // 隐藏文件被跳过（ignore.txt 是合法 txt，应计入）
        assert!(b.children.iter().all(|c|c.name!=".h.txt"));
        let _=fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tags_roundtrip_and_search(){
        let db=Database::open(":memory:".into()).unwrap();
        db.conn.lock().execute("INSERT INTO libraries(id,root_path,name,created_at) VALUES('l1','/tmp/x','t',1)",[]).unwrap();
        db.upsert_documents(&[entry("a.txt",1)]).unwrap();
        let id=db.documents().unwrap()[0].id.clone();
        // 初始无标签
        assert!(db.stored_document(&id).unwrap().summary.tags.is_empty());
        // 保存标签
        let s=db.update_tags(&id,&["爽文".into(),"重生".into()]).unwrap();
        assert_eq!(s.tags,vec!["爽文","重生"]);
        // documents() 也带标签
        assert_eq!(db.documents().unwrap()[0].tags.len(),2);
        // 标签搜索命中
        let hits=db.search(SearchQuery{text:String::new(),tag:Some("重生".into()),library_id:None,length_kind:None,purpose:None,progress:None,format:None}).unwrap();
        assert_eq!(hits.len(),1);
        assert_eq!(hits[0].document.id,id);
        // 覆盖保存（去重 + 清空）
        let s2=db.update_tags(&id,&["爽文".into(),"爽文".into()]).unwrap();
        assert_eq!(s2.tags,vec!["爽文"]);
    }
}
