//! IR → Markdown 렌더러 — README §6.
//!
//! 렌더러는 IR 만 보고 단순 `match` 분기로 Markdown 을 생성한다.
//! 출력은 **결정적(deterministic)** — 같은 IR 은 항상 같은 문자열을 낸다 (README §1-2).

use crate::ir::*;

/// IR 을 벡터 DB 친화적 Markdown 문자열로 렌더링한다.
pub struct MarkdownRenderer;

impl MarkdownRenderer {
    /// 문서 전체를 frontmatter 포함 Markdown 으로 렌더링.
    pub fn render(doc: &Document) -> String {
        let mut out = String::new();
        out.push_str(&Self::render_frontmatter(&doc.metadata));
        for block in &doc.blocks {
            out.push_str(&Self::render_block(block));
            out.push('\n');
        }
        // 말미 공백 정리 — 결정적 출력 보장.
        while out.ends_with('\n') {
            out.pop();
        }
        out.push('\n');
        out
    }

    /// frontmatter 없이 본문 블록만 렌더링 (테스트/조합용).
    pub fn render_blocks(blocks: &[Block]) -> String {
        let mut out = String::new();
        for block in blocks {
            out.push_str(&Self::render_block(block));
            out.push('\n');
        }
        out
    }

    fn render_frontmatter(meta: &DocumentMetadata) -> String {
        // 벡터 DB 메타데이터로 그대로 사용 가능한 YAML frontmatter.
        let mut fm = String::from("---\n");
        fm.push_str(&format!("title: {}\n", yaml_scalar(meta.title.as_deref().unwrap_or(""))));
        fm.push_str(&format!("author: {}\n", yaml_scalar(meta.author.as_deref().unwrap_or(""))));
        fm.push_str(&format!("source_format: {}\n", format!("{:?}", meta.source_format).to_lowercase()));
        fm.push_str(&format!("original_filename: {}\n", yaml_scalar(&meta.original_filename)));
        if let Some(p) = meta.page_count {
            fm.push_str(&format!("page_count: {}\n", p));
        }
        if let Some(lang) = &meta.language {
            fm.push_str(&format!("language: {}\n", yaml_scalar(lang)));
        }
        if let Some(ts) = &meta.created_at {
            fm.push_str(&format!("created_at: {}\n", ts.to_rfc3339()));
        }
        fm.push_str("---\n\n");
        fm
    }

    fn render_block(block: &Block) -> String {
        match block {
            Block::Heading { level, text } => {
                let lvl = (*level).clamp(1, 6) as usize;
                format!("{} {}\n", "#".repeat(lvl), collapse_ws(text))
            }
            Block::Paragraph(inlines) => {
                let s = render_inlines(inlines);
                if s.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}\n", s)
                }
            }
            Block::Table(t) => Self::render_table(t),
            Block::List { ordered, items } => Self::render_list(*ordered, items, 0),
            Block::CodeBlock { lang, code } => {
                let fence = pick_fence(code);
                format!("{f}{l}\n{c}\n{f}\n", f = fence, l = lang.as_deref().unwrap_or(""), c = code.trim_end_matches('\n'))
            }
            Block::Quote(inlines) => {
                let s = render_inlines(inlines);
                s.lines()
                    .map(|l| format!("> {}", l))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            }
            Block::HorizontalRule => "---\n".to_string(),
            Block::Image { alt, data } => format!("![{}]({})\n", escape_inline(alt), image_ref(data)),
            Block::Math { latex, display: true } => format!("$$\n{}\n$$\n", latex.trim()),
            Block::Math { latex, display: false } => format!("${}$\n", latex.trim()),
            Block::PageBreak => "\n---\n".to_string(),
            Block::Footnote { id, content } => {
                format!("[^{}]: {}\n", id, render_inlines(content))
            }
        }
    }

    fn render_table(t: &Table) -> String {
        let mut out = String::new();
        if let Some(cap) = &t.caption {
            out.push_str(&format!("**{}**\n\n", escape_inline(cap)));
        }
        // 헤더가 비어 있으면 첫 행을 헤더로 승격 (Markdown 표는 헤더 필수).
        let col_count = t
            .headers
            .len()
            .max(t.rows.iter().map(Vec::len).max().unwrap_or(0));
        if col_count == 0 {
            return out;
        }

        let header = pad_row(&t.headers, col_count);
        out.push_str(&format!("| {} |\n", header.iter().map(|c| escape_cell(c)).collect::<Vec<_>>().join(" | ")));
        out.push_str(&format!("| {} |\n", vec!["---"; col_count].join(" | ")));
        for row in &t.rows {
            let cells = pad_row(row, col_count);
            out.push_str(&format!("| {} |\n", cells.iter().map(|c| escape_cell(c)).collect::<Vec<_>>().join(" | ")));
        }
        out
    }

    fn render_list(ordered: bool, items: &[ListItem], depth: usize) -> String {
        let indent = "  ".repeat(depth);
        let mut out = String::new();
        for (i, item) in items.iter().enumerate() {
            let marker = if ordered {
                format!("{}.", i + 1)
            } else {
                "-".to_string()
            };
            out.push_str(&format!("{}{} {}\n", indent, marker, render_inlines(&item.content)));
            if let Some(sub) = &item.sublist {
                out.push_str(&Self::render_list(sub.ordered, &sub.items, depth + 1));
            }
        }
        out
    }
}

// ── 인라인 렌더링 ─────────────────────────────────────────────

fn render_inlines(inlines: &[Inline]) -> String {
    inlines.iter().map(render_inline).collect()
}

fn render_inline(inline: &Inline) -> String {
    match inline {
        Inline::Text(s) => escape_inline(s),
        Inline::Bold(inner) => format!("**{}**", render_inline(inner)),
        Inline::Italic(inner) => format!("*{}*", render_inline(inner)),
        Inline::Strike(inner) => format!("~~{}~~", render_inline(inner)),
        Inline::Code(s) => format!("`{}`", s),
        Inline::Link { text, url } => format!("[{}]({})", escape_inline(text), url),
        Inline::LineBreak => "  \n".to_string(),
    }
}

fn image_ref(data: &ImageData) -> String {
    match data {
        ImageData::Base64 { mime, data } => format!("data:{};base64,{}", mime, data),
        ImageData::Path(p) => p.clone(),
        ImageData::Url(u) => u.clone(),
    }
}

// ── 텍스트 유틸 ──────────────────────────────────────────────

/// 연속 공백/개행을 단일 공백으로 접는다 (헤딩/표 셀용).
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markdown 표 셀: `|` 와 개행 이스케이프.
fn escape_cell(s: &str) -> String {
    collapse_ws(s).replace('|', "\\|")
}

/// 일반 인라인 텍스트의 최소 이스케이프 (표 구분자/링크 깨짐 방지 수준).
fn escape_inline(s: &str) -> String {
    s.replace('|', "\\|")
}

/// 행 길이를 `col_count` 로 패딩.
fn pad_row(row: &[String], col_count: usize) -> Vec<String> {
    let mut v: Vec<String> = row.to_vec();
    while v.len() < col_count {
        v.push(String::new());
    }
    v
}

/// YAML 스칼라 안전 인용 — 콜론/특수문자 포함 시 큰따옴표로 감싼다.
fn yaml_scalar(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quote = s.contains(':')
        || s.contains('#')
        || s.contains('"')
        || s.starts_with(|c: char| c.is_whitespace())
        || s.ends_with(|c: char| c.is_whitespace())
        || s.starts_with(['-', '?', '*', '&', '!', '%', '@', '`', '[', '{', '>', '|']);
    if needs_quote {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// 코드블록이 백틱을 포함하면 더 긴 펜스를 고른다.
fn pick_fence(code: &str) -> String {
    let max_run = code
        .split('`')
        .count()
        .saturating_sub(1); // 대략적 백틱 존재 여부
    if max_run > 0 && code.contains("```") {
        "````".to_string()
    } else {
        "```".to_string()
    }
}
