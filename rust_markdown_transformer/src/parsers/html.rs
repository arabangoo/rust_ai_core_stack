//! HTML 파서 — README §5-4. `scraper`(html5ever 기반) DOM 순회 → IR.
//!
//! - `<article>` → `<main>` → `<body>` 순으로 주 콘텐츠 컨테이너를 선택해 boilerplate 를 줄인다.
//! - `<script>/<style>/<nav>/<header>/<footer>/<aside>` 등은 건너뛴다.
//! - 블록 요소(h1~h6/p/ul/ol/table/pre/blockquote/hr/img) → 대응 IR Block.
//! - 인라인 요소(b/strong, i/em, del/s, code, a, br) → 대응 IR Inline.

use std::io::Read;

use scraper::{ElementRef, Html, Node};

use crate::error::ParseError;
use crate::ir::*;
use crate::registry::FormatParser;

pub struct HtmlParser;

impl FormatParser for HtmlParser {
    fn supported_extensions(&self) -> &[&str] {
        &["html", "htm", "xhtml"]
    }

    fn name(&self) -> &'static str {
        "html"
    }

    fn can_parse_bytes(&self, header: &[u8]) -> bool {
        let s = String::from_utf8_lossy(header).to_ascii_lowercase();
        let s = s.trim_start();
        s.starts_with("<!doctype html") || s.starts_with("<html") || s.starts_with("<?xml")
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;
        let mut html = String::from_utf8_lossy(&buf).into_owned();
        if html.starts_with('\u{feff}') {
            html.remove(0); // UTF-8 BOM 제거
        }
        let doc = Html::parse_document(&html);

        let mut metadata = DocumentMetadata::new(SourceFormat::Html, filename);
        metadata.title = select_text(&doc, "title");

        let root = select_first(&doc, "article")
            .or_else(|| select_first(&doc, "main"))
            .or_else(|| select_first(&doc, "body"))
            .unwrap_or_else(|| doc.root_element());

        let mut ctx = BlockCtx::default();
        walk_blocks(root, &mut ctx);
        ctx.flush_paragraph();

        Ok(Document { metadata, blocks: ctx.blocks })
    }
}

#[derive(Default)]
struct BlockCtx {
    blocks: Vec<Block>,
    inline_buf: Vec<Inline>,
}

impl BlockCtx {
    fn flush_paragraph(&mut self) {
        if self.inline_buf.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut self.inline_buf);
        if plain(&buf).trim().is_empty() {
            return;
        }
        self.blocks.push(Block::Paragraph(buf));
    }
}

/// 블록 레벨 순회 — 텍스트/인라인은 inline_buf 에 모으고, 블록 요소를 만나면 flush.
fn walk_blocks(el: ElementRef, ctx: &mut BlockCtx) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => push_inline_text(&mut ctx.inline_buf, t),
            Node::Element(_) => {
                let Some(child_el) = ElementRef::wrap(child) else { continue };
                let name = child_el.value().name();
                if is_skipped(name) {
                    continue;
                }
                if is_inline(name) {
                    collect_inlines(child_el, &mut ctx.inline_buf);
                } else {
                    ctx.flush_paragraph();
                    handle_block(child_el, name, ctx);
                }
            }
            _ => {}
        }
    }
}

fn handle_block(el: ElementRef, name: &str, ctx: &mut BlockCtx) {
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<u8>().unwrap_or(1);
            let text = collapse(&element_plain(el));
            if !text.is_empty() {
                ctx.blocks.push(Block::Heading { level, text });
            }
        }
        "p" => {
            let mut inl = Vec::new();
            collect_inlines(el, &mut inl);
            if !plain(&inl).trim().is_empty() {
                ctx.blocks.push(Block::Paragraph(inl));
            }
        }
        "ul" | "ol" => {
            let ordered = name == "ol";
            let items = parse_list_items(el);
            if !items.is_empty() {
                ctx.blocks.push(Block::List { ordered, items });
            }
        }
        "table" => {
            if let Some(table) = parse_table(el) {
                ctx.blocks.push(Block::Table(table));
            }
        }
        "pre" => {
            let (lang, code) = parse_pre(el);
            ctx.blocks.push(Block::CodeBlock { lang, code });
        }
        "blockquote" => {
            let mut inl = Vec::new();
            collect_inlines(el, &mut inl);
            if !plain(&inl).trim().is_empty() {
                ctx.blocks.push(Block::Quote(inl));
            }
        }
        "hr" => ctx.blocks.push(Block::HorizontalRule),
        "img" => {
            if let Some(img) = parse_img(el) {
                ctx.blocks.push(img);
            }
        }
        "br" => {} // 블록 컨텍스트의 br 은 무시
        // div/section/그 외 컨테이너 → 내부로 재귀.
        _ => walk_blocks(el, ctx),
    }
}

fn parse_list_items(list: ElementRef) -> Vec<ListItem> {
    let mut items = Vec::new();
    for child in list.children() {
        let Some(li) = ElementRef::wrap(child) else { continue };
        if li.value().name() != "li" {
            continue;
        }
        // li 직속 인라인 + 중첩 ul/ol 분리.
        let mut content = Vec::new();
        let mut sublist: Option<Box<NestedList>> = None;
        for c in li.children() {
            match c.value() {
                Node::Text(t) => push_inline_text(&mut content, t),
                Node::Element(_) => {
                    let Some(ce) = ElementRef::wrap(c) else { continue };
                    let cn = ce.value().name();
                    if cn == "ul" || cn == "ol" {
                        sublist = Some(Box::new(NestedList {
                            ordered: cn == "ol",
                            items: parse_list_items(ce),
                        }));
                    } else if is_inline(cn) {
                        collect_inlines(ce, &mut content);
                    } else {
                        // li 안의 블록(p 등) → 평문 인라인으로 흡수.
                        collect_inlines(ce, &mut content);
                    }
                }
                _ => {}
            }
        }
        if !plain(&content).trim().is_empty() || sublist.is_some() {
            items.push(ListItem { content, sublist });
        }
    }
    items
}

fn parse_table(table: ElementRef) -> Option<Table> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    // tr 을 깊이 무관하게 수집 (thead/tbody/tfoot 관통).
    for tr in descendants_named(table, "tr") {
        let mut cells = Vec::new();
        for cell in tr.children() {
            let Some(ce) = ElementRef::wrap(cell) else { continue };
            let n = ce.value().name();
            if n == "td" || n == "th" {
                cells.push(collapse(&element_plain(ce)));
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return None;
    }
    let headers = rows.remove(0);
    Some(Table { headers, rows, caption: None })
}

fn parse_pre(pre: ElementRef) -> (Option<String>, String) {
    // <code class="language-xxx"> 에서 언어 추출.
    let mut lang = None;
    for d in pre.descendants() {
        if let Some(ce) = ElementRef::wrap(d) {
            if ce.value().name() == "code" {
                if let Some(class) = ce.value().attr("class") {
                    lang = class
                        .split_whitespace()
                        .find_map(|c| c.strip_prefix("language-").or_else(|| c.strip_prefix("lang-")))
                        .map(|s| s.to_string());
                }
                break;
            }
        }
    }
    let code = element_plain(pre).trim_end().to_string();
    (lang, code)
}

fn parse_img(el: ElementRef) -> Option<Block> {
    let src = el.value().attr("src")?;
    let alt = el.value().attr("alt").unwrap_or("").to_string();
    let data = if src.starts_with("http://") || src.starts_with("https://") {
        ImageData::Url(src.to_string())
    } else if let Some(rest) = src.strip_prefix("data:") {
        // data:image/png;base64,XXXX
        if let Some((meta, b64)) = rest.split_once(";base64,") {
            ImageData::Base64 { mime: meta.to_string(), data: b64.to_string() }
        } else {
            ImageData::Url(src.to_string())
        }
    } else {
        ImageData::Path(src.to_string())
    };
    Some(Block::Image { alt, data })
}

// ── 인라인 수집 ──────────────────────────────────────────────

fn collect_inlines(el: ElementRef, out: &mut Vec<Inline>) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => push_inline_text(out, t),
            Node::Element(_) => {
                let Some(ce) = ElementRef::wrap(child) else { continue };
                let name = ce.value().name();
                if is_skipped(name) {
                    continue;
                }
                match name {
                    "b" | "strong" => {
                        let mut inner = Vec::new();
                        collect_inlines(ce, &mut inner);
                        wrap_each(inner, out, Inline::Bold);
                    }
                    "i" | "em" => {
                        let mut inner = Vec::new();
                        collect_inlines(ce, &mut inner);
                        wrap_each(inner, out, Inline::Italic);
                    }
                    "del" | "s" | "strike" => {
                        let mut inner = Vec::new();
                        collect_inlines(ce, &mut inner);
                        wrap_each(inner, out, Inline::Strike);
                    }
                    "code" => {
                        let code = element_plain(ce);
                        if !code.is_empty() {
                            out.push(Inline::Code(code));
                        }
                    }
                    "a" => {
                        let text = collapse(&element_plain(ce));
                        let url = ce.value().attr("href").unwrap_or("").to_string();
                        if !text.is_empty() {
                            out.push(Inline::Link { text, url });
                        }
                    }
                    "br" => out.push(Inline::LineBreak),
                    // span/u/mark/sub/sup/small/font/그 외 → 그대로 내부 수집.
                    _ => collect_inlines(ce, out),
                }
            }
            _ => {}
        }
    }
}

fn wrap_each(inner: Vec<Inline>, out: &mut Vec<Inline>, f: impl Fn(Box<Inline>) -> Inline) {
    for i in inner {
        out.push(f(Box::new(i)));
    }
}

/// HTML 공백 규칙: 연속 공백/개행을 단일 공백으로 접되 경계 공백은 보존.
fn push_inline_text(out: &mut Vec<Inline>, t: &str) {
    if t.is_empty() {
        return;
    }
    let leading = t.starts_with(|c: char| c.is_whitespace());
    let trailing = t.ends_with(|c: char| c.is_whitespace());
    let core = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if core.is_empty() {
        // 순수 공백 텍스트 — 인접 단어 분리를 위해 단일 공백만 (버퍼가 비어있지 않을 때).
        if !out.is_empty() {
            out.push(Inline::Text(" ".to_string()));
        }
        return;
    }
    let mut s = String::new();
    if leading && !out.is_empty() {
        s.push(' ');
    }
    s.push_str(&core);
    if trailing {
        s.push(' ');
    }
    out.push(Inline::Text(s));
}

// ── 평문 추출 유틸 ──────────────────────────────────────────

fn element_plain(el: ElementRef) -> String {
    let mut s = String::new();
    for t in el.text() {
        s.push_str(t);
    }
    s
}

fn plain(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Bold(b) | Inline::Italic(b) | Inline::Strike(b) => {
                s.push_str(&plain(std::slice::from_ref(b)))
            }
            Inline::Link { text, .. } => s.push_str(text),
            Inline::LineBreak => s.push(' '),
        }
    }
    s
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── 선택자 헬퍼 ─────────────────────────────────────────────

fn descendants_named<'a>(el: ElementRef<'a>, name: &str) -> Vec<ElementRef<'a>> {
    el.descendants()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == name)
        .collect()
}

fn select_first<'a>(doc: &'a Html, selector: &str) -> Option<ElementRef<'a>> {
    let sel = scraper::Selector::parse(selector).ok()?;
    doc.select(&sel).next()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    let el = select_first(doc, selector)?;
    let txt = collapse(&element_plain(el));
    if txt.is_empty() {
        None
    } else {
        Some(txt)
    }
}

fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "b" | "strong"
            | "i" | "em"
            | "del" | "s" | "strike"
            | "code"
            | "a"
            | "br"
            | "span" | "u" | "mark" | "sub" | "sup" | "small" | "font" | "abbr" | "cite" | "q" | "time"
    )
}

fn is_skipped(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "noscript" | "template" | "nav" | "header" | "footer" | "aside" | "head"
    )
}
