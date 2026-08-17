// 导出 Word：将 txt / md 文稿生成为 .docx（宋体小四、1.5 倍行距、正文首行缩进 2 字符）。
// 章节行「###1.」「###2」自动转换为「第一章」「第二章」（支持 1-999 中文数字）。
// 直接构造最小 OOXML 包，不引入额外前端依赖。

use crate::documents;
use crate::models::*;
use crate::AppState;
use regex::Regex;
use std::io::Write;
use std::sync::Arc;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 去掉行内 Markdown 标记（**加粗**、*斜体*、`代码`、链接、图片），保留文字本身。
fn clean_inline(text: &str) -> String {
    let re = Regex::new(
        r"!\[([^\]]*)\]\([^)]*\)|\*\*([^*]+)\*\*|\*([^*]+)\*|`([^`]+)`|\[([^\]]+)\]\([^)]*\)",
    )
    .unwrap();
    re.replace_all(text.trim(), |caps: &regex::Captures| {
        caps.get(2)
            .or(caps.get(3))
            .or(caps.get(4))
            .or(caps.get(5))
            .or(caps.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default()
    })
    .into_owned()
}

/// 阿拉伯数字 → 中文数字（1-999，如 1→一、21→二十一、105→一百零五）。
fn to_chinese_num(n: u32) -> String {
    const D: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    if n == 0 {
        return "零".into();
    }
    let mut out = String::new();
    if n >= 100 {
        out.push_str(D[(n / 100) as usize]);
        out.push('百');
        let rest = n % 100;
        if rest != 0 && rest < 10 {
            out.push('零');
        }
    }
    let rest = n % 100;
    if rest >= 10 {
        let tens = rest / 10;
        // 10-19 的十位「一」省略：十、十一、十九
        let omit_one = n < 100 && tens == 1;
        if !omit_one {
            out.push_str(D[tens as usize]);
        }
        out.push('十');
        if !rest.is_multiple_of(10) {
            out.push_str(D[(rest % 10) as usize]);
        }
    } else if rest > 0 {
        out.push_str(D[rest as usize]);
    }
    out
}

enum Para {
    /// 章节标题（第N章）：居中、加粗、四号
    Heading(String),
    /// Markdown 标题行（# 开头非数字章节）：加粗、不缩进
    Title(String),
    /// 正文：宋体小四、首行缩进 2 字符
    Body(String),
    /// 空段：保留段间距
    Blank,
}

/// 解析文稿文本为段落序列。包含空行时按空行分段（标准 Markdown），
/// 无空行时每行一段（txt / 正文.md 常见写法）。
fn parse_paragraphs(text: &str) -> Vec<Para> {
    let chapter = Regex::new(r"^###\s*(\d{1,3})\.?\s*$").unwrap();
    let heading = Regex::new(r"^#{1,6}\s+(.*)$").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let has_blank = lines.iter().any(|line| line.trim().is_empty());
    let mut out: Vec<Para> = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Para>| {
        if !buf.trim().is_empty() {
            out.push(Para::Body(clean_inline(buf)));
            buf.clear();
        }
    };
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if has_blank {
                flush(&mut buf, &mut out);
                out.push(Para::Blank);
            }
            continue;
        }
        if let Some(caps) = chapter.captures(trimmed) {
            flush(&mut buf, &mut out);
            if let Ok(n) = caps[1].parse::<u32>() {
                out.push(Para::Heading(format!("第{}章", to_chinese_num(n))));
            }
            continue;
        }
        if has_blank {
            if let Some(caps) = heading.captures(trimmed) {
                flush(&mut buf, &mut out);
                out.push(Para::Title(clean_inline(&caps[1])));
                continue;
            }
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(trimmed);
        } else {
            flush(&mut buf, &mut out);
            out.push(Para::Body(clean_inline(trimmed)));
        }
    }
    flush(&mut buf, &mut out);
    out
}

const RUN: &str = "<w:rPr><w:rFonts w:ascii=\"宋体\" w:eastAsia=\"宋体\" w:hAnsi=\"宋体\"/><w:b/><w:sz w:val=\"28\"/><w:szCs w:val=\"28\"/></w:rPr>";
const RUN_BODY: &str = "<w:rPr><w:rFonts w:ascii=\"宋体\" w:eastAsia=\"宋体\" w:hAnsi=\"宋体\"/><w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/></w:rPr>";

/// 文档顶部的小说标题：居中、加粗、二号（22pt）。
fn title_para_xml(title: &str) -> String {
    let run = "<w:rPr><w:rFonts w:ascii=\"宋体\" w:eastAsia=\"宋体\" w:hAnsi=\"宋体\"/><w:b/><w:sz w:val=\"44\"/><w:szCs w:val=\"44\"/></w:rPr>";
    format!(
        "<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:jc w:val=\"center\"/>{run}</w:pPr><w:r>{run}<w:t>{}</w:t></w:r></w:p>",
        escape_xml(title)
    )
}

fn para_xml(kind: &Para) -> String {
    match kind {
        Para::Heading(title) => format!(
            "<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:jc w:val=\"center\"/>{RUN}</w:pPr><w:r>{RUN}<w:t>{}</w:t></w:r></w:p>",
            escape_xml(title)
        ),
        Para::Title(title) => format!(
            "<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/>{RUN_BODY}</w:pPr><w:r><w:rPr><w:rFonts w:ascii=\"宋体\" w:eastAsia=\"宋体\" w:hAnsi=\"宋体\"/><w:b/><w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/></w:rPr><w:t>{}</w:t></w:r></w:p>",
            escape_xml(title)
        ),
        Para::Body(text) => format!(
            "<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/><w:ind w:firstLineChars=\"200\" w:firstLine=\"480\"/>{RUN_BODY}</w:pPr><w:r>{RUN_BODY}<w:t>{}</w:t></w:r></w:p>",
            escape_xml(text)
        ),
        Para::Blank => "<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/></w:pPr></w:p>".into(),
    }
}

fn build_docx(document_xml: &str) -> Result<Vec<u8>, AppError> {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="宋体" w:eastAsia="宋体" w:hAnsi="宋体"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:line="360" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults></w:styles>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{document_xml}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#
    );
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    let mut put = |name: &str, data: &str| -> Result<(), AppError> {
        zip.start_file(name, opts)
            .map_err(|e| AppError::Message(format!("写入 docx 失败：{e}")))?;
        zip.write_all(data.as_bytes())
            .map_err(|e| AppError::Message(format!("写入 docx 失败：{e}")))?;
        Ok(())
    };
    put("[Content_Types].xml", content_types)?;
    put("_rels/.rels", rels)?;
    put("word/_rels/document.xml.rels", doc_rels)?;
    put("word/document.xml", &document)?;
    put("word/styles.xml", styles)?;
    let cursor = zip
        .finish()
        .map_err(|e| AppError::Message(format!("生成 docx 失败：{e}")))?;
    Ok(cursor.into_inner())
}

/// 由相对路径解析小说标题：文件名为「正文.md / 正文.txt」时取父文件夹名，否则取文件名（去扩展名）。
fn resolve_title(relative: &str) -> String {
    let parts: Vec<&str> = relative.split('/').collect();
    let file_name = parts.last().copied().unwrap_or("");
    let base = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    let is_zhengwen = base == "正文" && (file_name.ends_with(".md") || file_name.ends_with(".txt"));
    if is_zhengwen && parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        base.to_string()
    }
}

/// 将文稿导出为 Word 文档，输出到源文件所在目录，文件名为「{小说标题}.docx」，
/// 文档顶部居中写入小说标题。返回输出路径。
pub fn export(state: &Arc<AppState>, document_id: &str) -> Result<String, AppError> {
    let stored = state.db.stored_document(document_id)?;
    let relative = &stored.summary.relative_path;
    let title = resolve_title(relative);
    let parts: Vec<&str> = relative.split('/').collect();
    let parent = if parts.len() > 1 {
        parts[..parts.len() - 1].join("/")
    } else {
        String::new()
    };
    let dir = if parent.is_empty() {
        stored.root_path.clone()
    } else {
        documents::safe_path(&stored.root_path, &parent)?
    };
    let output = dir.join(format!("{title}.docx"));
    let content = documents::read(state, document_id)?;
    let paras = parse_paragraphs(&content.content);
    let mut body = String::new();
    body.push_str(&title_para_xml(&title));
    body.push_str("<w:p><w:pPr><w:spacing w:line=\"360\" w:lineRule=\"auto\"/></w:pPr></w:p>");
    for para in &paras {
        body.push_str(&para_xml(para));
    }
    let bytes = build_docx(&body)?;
    std::fs::write(&output, &bytes)
        .map_err(|e| AppError::Message(format!("保存 Word 文档失败：{e}")))?;
    Ok(output.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_numbers() {
        assert_eq!(to_chinese_num(1), "一");
        assert_eq!(to_chinese_num(2), "二");
        assert_eq!(to_chinese_num(10), "十");
        assert_eq!(to_chinese_num(11), "十一");
        assert_eq!(to_chinese_num(21), "二十一");
        assert_eq!(to_chinese_num(100), "一百");
        assert_eq!(to_chinese_num(105), "一百零五");
        assert_eq!(to_chinese_num(999), "九百九十九");
    }

    #[test]
    fn parses_chapter_lines() {
        let paras = parse_paragraphs("###1.\n正文第一段。\n###2\n正文第二段。");
        assert_eq!(paras.len(), 4);
        match &paras[0] {
            Para::Heading(t) => assert_eq!(t, "第一章"),
            _ => panic!("expected heading"),
        }
        match &paras[1] {
            Para::Body(t) => assert_eq!(t, "正文第一段。"),
            _ => panic!("expected body"),
        }
        match &paras[2] {
            Para::Heading(t) => assert_eq!(t, "第二章"),
            _ => panic!("expected heading"),
        }
    }

    #[test]
    fn no_blank_lines_means_each_line_is_a_paragraph() {
        let paras = parse_paragraphs("第一段。\n第二段。\n**加粗** 和 `代码`。");
        assert_eq!(paras.len(), 3);
        match &paras[2] {
            Para::Body(t) => assert_eq!(t, "加粗 和 代码。"),
            _ => panic!("expected body"),
        }
    }

    #[test]
    fn blank_lines_group_paragraphs_and_titles() {
        let paras = parse_paragraphs("# 标题\n\n第一行\n第二行。");
        assert!(matches!(paras[0], Para::Title(_)));
        match &paras[1] {
            Para::Blank => {}
            _ => panic!("expected blank"),
        }
        match &paras[2] {
            Para::Body(t) => assert_eq!(t, "第一行 第二行。"),
            _ => panic!("expected body"),
        }
    }

    #[test]
    fn resolves_novel_title_from_path() {
        assert_eq!(resolve_title("山雨退婚/正文.md"), "山雨退婚");
        assert_eq!(resolve_title("山雨退婚/正文.txt"), "山雨退婚");
        assert_eq!(resolve_title("山雨退婚/设定.md"), "设定");
        assert_eq!(resolve_title("我的小说.txt"), "我的小说");
        assert_eq!(resolve_title("正文.md"), "正文");
        assert_eq!(resolve_title("a/b/正文.md"), "b");
    }

    #[test]
    fn docx_is_a_valid_package() {
        let paras = parse_paragraphs("###1.\n正文第一段。\n###2\n正文第二段。");
        let body: String = paras.iter().map(para_xml).collect();
        let bytes = build_docx(&body).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(archive.len(), 5);
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/styles.xml",
        ] {
            assert!(archive.by_name(name).is_ok(), "缺少 {name}");
        }
        let mut document = archive.by_name("word/document.xml").unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut document, &mut xml).unwrap();
        assert!(xml.contains("第一章"));
        assert!(xml.contains("w:line=\"360\""));
        assert!(xml.contains("宋体"));
    }
}
