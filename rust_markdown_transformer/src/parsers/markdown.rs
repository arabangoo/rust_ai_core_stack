//! Markdown 파서/재정규화기 — README §5-4. `pulldown-cmark` 이벤트 → IR.
//!
//! 이미 Markdown 인 파일도 IR 을 거쳐 **재정규화**한다 (헤딩 레벨/표 정렬/공백 정리).
//! 그 결과 다른 포맷에서 온 산출물과 동일한 형태로 통일된다.

use std::io::Read;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::error::ParseError;
use crate::ir::*;
use crate::registry::FormatParser;

const FMT: &str = "markdown";

pub struct MarkdownParser;

impl FormatParser for MarkdownParser {
    fn supported_extensions(&self) -> &[&str] {
        &["md", "markdown", "mdown", "mkd"]
    }

    fn name(&self) -> &'static str {
        "markdown"
    }

    fn can_parse_bytes(&self, _header: &[u8]) -> bool {
        // Markdown 은 매직바이트가 없다 → 확장자로만 디스패치.
        false
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;
        let mut text = String::from_utf8(buf)
            .map_err(|e| ParseError::encoding(FMT, e.to_string()))?;
        if text.starts_with('\u{feff}') {
            text.remove(0); // UTF-8 BOM 제거
        }

        let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let mut st = MdState::default();
        for ev in Parser::new_ext(&text, opts) {
            st.handle(ev);
        }
        st.flush_paragraph();

        Ok(Document {
            metadata: DocumentMetadata::new(SourceFormat::Markdown, filename),
            blocks: st.blocks,
        })
    }
}

#[derive(Default)]
struct MdState {
    blocks: Vec<Block>,
    inline: Vec<Inline>,
    bold: u32,
    italic: u32,
    strike: u32,
    heading: Option<u8>,
    in_quote: bool,
    code: Option<(Option<String>, String)>,
    link: Option<(String, String)>,  // (url, text)
    image: Option<(String, String)>, // (url, alt)
    lists: Vec<ListBuild>,
    table: Option<TableBuild>,
}

struct ListBuild {
    ordered: bool,
    items: Vec<ListItem>,
    cur: Vec<Inline>,
    cur_sublist: Option<Box<NestedList>>,
}

#[derive(Default)]
struct TableBuild {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    cur_row: Vec<String>,
    cell: String,
    in_head: bool,
}

impl MdState {
    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                if self.link.is_some() || self.image.is_some() {
                    self.text(&t);
                } else {
                    self.push_inline(Inline::Code(t.to_string()));
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.push_inline(Inline::LineBreak),
            Event::Rule => {
                self.flush_paragraph();
                self.blocks.push(Block::HorizontalRule);
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                self.push_inline(Inline::Text(mark.to_string()));
            }
            _ => {} // Html/InlineHtml/footnote/math 등은 v0.1 미처리
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_paragraph();
                self.inline.clear();
                self.heading = Some(level as u8);
            }
            Tag::Paragraph => {}
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.in_quote = true;
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let l = info.split_whitespace().next().unwrap_or("").to_string();
                        if l.is_empty() { None } else { Some(l) }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.flush_paragraph();
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush_paragraph();
                self.lists.push(ListBuild {
                    ordered: start.is_some(),
                    items: Vec::new(),
                    cur: Vec::new(),
                    cur_sublist: None,
                });
            }
            Tag::Item => {
                if let Some(lb) = self.lists.last_mut() {
                    lb.cur = Vec::new();
                    lb.cur_sublist = None;
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some((dest_url.to_string(), String::new())),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            Tag::Table(_) => self.table = Some(TableBuild::default()),
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                    t.cur_row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.cur_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(t) = &mut self.table {
                    t.cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let text = plain(&self.inline).split_whitespace().collect::<Vec<_>>().join(" ");
                let level = self.heading.take().unwrap_or(1);
                if !text.is_empty() {
                    self.blocks.push(Block::Heading { level, text });
                }
                self.inline.clear();
            }
            TagEnd::Paragraph => {
                if !self.lists.is_empty() {
                    // 같은 항목 내 여러 단락 → 공백 구분.
                    if let Some(lb) = self.lists.last_mut() {
                        if !lb.cur.is_empty() {
                            lb.cur.push(Inline::Text(" ".to_string()));
                        }
                    }
                } else if self.in_quote {
                    if !self.inline.is_empty() {
                        self.inline.push(Inline::Text(" ".to_string()));
                    }
                } else {
                    self.flush_paragraph();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.in_quote = false;
                let inl = std::mem::take(&mut self.inline);
                if !plain(&inl).trim().is_empty() {
                    self.blocks.push(Block::Quote(inl));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.blocks.push(Block::CodeBlock {
                        lang,
                        code: code.trim_end_matches('\n').to_string(),
                    });
                }
            }
            TagEnd::List(_) => {
                if let Some(lb) = self.lists.pop() {
                    let items = lb.items;
                    if let Some(parent) = self.lists.last_mut() {
                        parent.cur_sublist =
                            Some(Box::new(NestedList { ordered: lb.ordered, items }));
                    } else if !items.is_empty() {
                        self.blocks.push(Block::List { ordered: lb.ordered, items });
                    }
                }
            }
            TagEnd::Item => {
                if let Some(lb) = self.lists.last_mut() {
                    let content = std::mem::take(&mut lb.cur);
                    let sublist = lb.cur_sublist.take();
                    if !plain(&content).trim().is_empty() || sublist.is_some() {
                        lb.items.push(ListItem { content, sublist });
                    }
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some((url, text)) = self.link.take() {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        self.push_inline(Inline::Link { text, url });
                    }
                }
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    let data = if url.starts_with("http://") || url.starts_with("https://") {
                        ImageData::Url(url)
                    } else {
                        ImageData::Path(url)
                    };
                    if self.table.is_some() {
                        if let Some(t) = &mut self.table {
                            t.cell.push_str(&alt);
                        }
                    } else {
                        self.flush_paragraph();
                        self.blocks.push(Block::Image { alt, data });
                    }
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    let cell = t.cell.split_whitespace().collect::<Vec<_>>().join(" ");
                    t.cur_row.push(cell);
                    t.cell.clear();
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.headers = std::mem::take(&mut t.cur_row);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    if !t.in_head {
                        t.rows.push(std::mem::take(&mut t.cur_row));
                    }
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.blocks.push(Block::Table(Table {
                        headers: t.headers,
                        rows: t.rows,
                        caption: None,
                    }));
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, s: &str) {
        if let Some((_, buf)) = &mut self.code {
            buf.push_str(s);
            return;
        }
        if let Some((_, t)) = &mut self.link {
            t.push_str(s);
            return;
        }
        if let Some((_, a)) = &mut self.image {
            a.push_str(s);
            return;
        }
        let inl = self.styled(s.to_string());
        self.push_inline(inl);
    }

    fn styled(&self, s: String) -> Inline {
        let mut i = Inline::Text(s);
        if self.strike > 0 {
            i = Inline::Strike(Box::new(i));
        }
        if self.italic > 0 {
            i = Inline::Italic(Box::new(i));
        }
        if self.bold > 0 {
            i = Inline::Bold(Box::new(i));
        }
        i
    }

    fn push_inline(&mut self, inl: Inline) {
        if let Some(t) = &mut self.table {
            t.cell.push_str(&plain(std::slice::from_ref(&inl)));
        } else if let Some(lb) = self.lists.last_mut() {
            lb.cur.push(inl);
        } else {
            self.inline.push(inl);
        }
    }

    fn flush_paragraph(&mut self) {
        if self.heading.is_some() || self.code.is_some() || self.in_quote {
            return;
        }
        let inl = std::mem::take(&mut self.inline);
        if !plain(&inl).trim().is_empty() {
            self.blocks.push(Block::Paragraph(inl));
        }
    }
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
