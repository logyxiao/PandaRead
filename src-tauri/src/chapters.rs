use crate::models::ChapterNode;
use regex::Regex;
use uuid::Uuid;

use std::sync::OnceLock;

static LATIN: OnceLock<Regex> = OnceLock::new();

pub fn count_words(text: &str) -> i64 {
    let mut count = text
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count() as i64;
    let latin = LATIN.get_or_init(|| Regex::new(r"[A-Za-z0-9]+(?:['’-][A-Za-z0-9]+)*").unwrap());
    count += latin.find_iter(text).count() as i64;
    count
}

pub fn detect(document_id: &str, text: &str) -> Vec<ChapterNode> {
    let chapter = Regex::new(r"(?m)^[ \t]*(第[零〇一二两三四五六七八九十百千万0-9]+[章节卷回部集篇]|序章|楔子|引子|前言|后记|尾声|番外(?:[一二三四五六七八九十0-9]+)?|Chapter\s+\d+|[0-9]{1,4}[\.、]\s*[^\n]{0,40})[^\n]{0,60}$").unwrap();
    let mut out: Vec<ChapterNode> = chapter
        .find_iter(text)
        .map(|m| ChapterNode {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.into(),
            title: m.as_str().trim().into(),
            offset: utf16_offset(text, m.start()),
            kind: "auto".into(),
            level: if m.as_str().contains('卷') || m.as_str().contains('部') {
                0
            } else {
                1
            },
        })
        .collect();
    if out.len() < 2 && text.chars().count() > 2600 {
        out.clear();
        out.push(node(document_id, "开篇", 0));
        let separators =
            Regex::new(r"(?m)^\s*(?:[-—=＊*]{3,}|[〇零一二三四五六七八九十]{1,3})\s*$").unwrap();
        for m in separators.find_iter(text) {
            out.push(node(
                document_id,
                &format!("剧情节点 {}", out.len() + 1),
                utf16_offset(text, m.end()),
            ));
        }
        if out.len() < 2 {
            let mut target = 1800usize;
            for (offset, _) in text.match_indices("\n\n") {
                if offset >= target {
                    out.push(node(
                        document_id,
                        &format!("剧情节点 {}", out.len() + 1),
                        utf16_offset(text, offset),
                    ));
                    target = offset + 1800;
                }
            }
        }
    }
    out
}

fn utf16_offset(text: &str, byte_offset: usize) -> i64 {
    text[..byte_offset].encode_utf16().count() as i64
}

fn node(document_id: &str, title: &str, offset: i64) -> ChapterNode {
    ChapterNode {
        id: Uuid::new_v4().to_string(),
        document_id: document_id.into(),
        title: title.into(),
        offset,
        kind: "auto".into(),
        level: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_chinese_and_latin() {
        assert_eq!(count_words("你好 hello world 123"), 5);
    }
    #[test]
    fn detects_named_chapters() {
        let c = detect("x", "第一章 初见\n正文\n第二章 重逢\n正文");
        assert_eq!(c.len(), 2);
    }
    #[test]
    fn detects_short_nodes() {
        let text = (0..5)
            .map(|_| "段落文字".repeat(500))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert!(detect("x", &text).len() > 1);
    }
    #[test]
    fn offsets_use_javascript_utf16_units() {
        let text = "简介😀\n第一章 开始\n正文";
        let c = detect("x", text);
        assert_eq!(c[0].offset, "简介😀\n".encode_utf16().count() as i64);
    }
}
