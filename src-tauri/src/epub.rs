use crate::models::{AppError, ChapterNode};
use roxmltree::Document;
use scraper::{Html, Selector};
use std::{collections::HashMap, fs::File, io::Read, path::{Path, PathBuf}};
use uuid::Uuid;
use zip::ZipArchive;

pub struct ParsedEpub { pub text: String, pub chapters: Vec<ChapterNode> }

pub fn parse(path:&Path,document_id:&str)->Result<ParsedEpub,AppError>{
    let file=File::open(path)?;let mut zip=ZipArchive::new(file)?;
    if zip.by_name("META-INF/encryption.xml").is_ok(){return Err(AppError::Message("该 EPUB 已加密，熊猫阅读无法读取".into()));}
    let mut container=String::new();zip.by_name("META-INF/container.xml")?.read_to_string(&mut container)?;
    let doc=Document::parse(&container).map_err(|_|AppError::Message("EPUB 容器信息损坏".into()))?;
    let opf_path=doc.descendants().find(|n|n.has_tag_name("rootfile")).and_then(|n|n.attribute("full-path")).ok_or_else(||AppError::Message("EPUB 缺少内容索引".into()))?.to_string();
    let mut opf=String::new();zip.by_name(&opf_path)?.read_to_string(&mut opf)?;
    let opf_doc=Document::parse(&opf).map_err(|_|AppError::Message("EPUB 内容索引损坏".into()))?;
    let manifest:HashMap<String,String>=opf_doc.descendants().filter(|n|n.has_tag_name("item")).filter_map(|n|Some((n.attribute("id")?.to_string(),n.attribute("href")?.to_string()))).collect();
    let spine:Vec<String>=opf_doc.descendants().filter(|n|n.has_tag_name("itemref")).filter_map(|n|n.attribute("idref").map(str::to_string)).collect();
    let base=Path::new(&opf_path).parent().unwrap_or(Path::new(""));let mut text=String::new();let mut chapters=Vec::new();
    for id in spine { if let Some(href)=manifest.get(&id){let entry=normalize_zip(base.join(href));let mut html=String::new();let mut file=match zip.by_name(&entry){Ok(file)=>file,Err(_)=>continue};if file.read_to_string(&mut html).is_err(){continue;}drop(file);let fragment=Html::parse_document(&html);let body=Selector::parse("body").unwrap();let heading=Selector::parse("h1,h2,h3,title").unwrap();let title=fragment.select(&heading).next().map(|e|e.text().collect::<String>().trim().to_string()).filter(|s|!s.is_empty()).unwrap_or_else(||format!("章节 {}",chapters.len()+1));let offset=text.len() as i64;let part=fragment.select(&body).next().map(|e|e.text().collect::<Vec<_>>().join(" ")).unwrap_or_default();if !part.trim().is_empty(){chapters.push(ChapterNode{id:Uuid::new_v4().to_string(),document_id:document_id.into(),title,offset,kind:"auto".into(),level:1});text.push_str(part.trim());text.push_str("\n\n");}}}
    if text.is_empty(){return Err(AppError::Message("EPUB 没有可读取的正文".into()));}Ok(ParsedEpub{text,chapters})
}
fn normalize_zip(p:PathBuf)->String{p.components().fold(PathBuf::new(),|mut out,c|{use std::path::Component;if let Component::Normal(s)=c{out.push(s)}out}).to_string_lossy().replace('\\',"/")}
