//! DOCX 파서 — README §5-1.
//!
//! 전략(README §5-1 "난관: 스타일 매핑"):
//! 1. `word/styles.xml` 를 먼저 파싱해 **styleId → Heading 레벨** 맵을 만든다.
//! 2. `word/document.xml` 의 `<w:p>/<w:r>/<w:t>` 를 quick-xml 이벤트로 훑어
//!    `<w:pStyle>` 로 Heading/Paragraph/List 를, `<w:b>/<w:i>` 로 Bold/Italic 을,
//!    `<w:tbl>` 로 Table 을 만든다.
//! 3. `docProps/core.xml` 에서 title/author 메타데이터를 보강한다.

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::ParseError;
use crate::ir::*;
use crate::parsers::ooxml::OoxmlPackage;
use crate::registry::FormatParser;

const FMT: &str = "docx";

pub struct DocxParser;

impl FormatParser for DocxParser {
    fn supported_extensions(&self) -> &[&str] {
        &["docx"]
    }

    fn name(&self) -> &'static str {
        "docx"
    }

    fn can_parse_bytes(&self, header: &[u8]) -> bool {
        // ZIP 매직 PK\x03\x04 — OOXML 공통. 확장자 우선 디스패치이므로 폴백용.
        header.starts_with(&[0x50, 0x4B, 0x03, 0x04])
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let pkg = OoxmlPackage::from_reader(input, FMT)?;

        let styles = parse_styles(&pkg)?;
        let doc_xml = pkg
            .get_str("word/document.xml", FMT)?
            .ok_or_else(|| ParseError::container(FMT, "missing word/document.xml"))?;

        let mut metadata = DocumentMetadata::new(SourceFormat::Docx, filename);
        parse_core_props(&pkg, &mut metadata)?;

        // 본문 그림 관계 (`word/_rels/document.xml.rels`) → rId → 이미지.
        let images = pkg.image_rels("word/document.xml", FMT)?;
        let blocks = parse_body(&doc_xml, &styles, &images)?;
        Ok(Document { metadata, blocks })
    }
}

/// styleId → Heading 레벨 맵 구축.
fn parse_styles(pkg: &OoxmlPackage) -> Result<HashMap<String, u8>, ParseError> {
    let mut map = HashMap::new();
    let xml = match pkg.get_str("word/styles.xml", FMT)? {
        Some(x) => x,
        None => return Ok(map),
    };
    let mut reader = Reader::from_str(&xml);
    let mut cur_id: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) if e.name().as_ref() == b"w:style" => {
                cur_id = attr_val(&e, b"w:styleId");
            }
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"w:name" => {
                if let (Some(id), Some(name)) = (&cur_id, attr_val(&e, b"w:val")) {
                    if let Some(level) = heading_level(id, &name) {
                        map.insert(id.clone(), level);
                    }
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:style" => {
                // name 없이 styleId 만으로도 판별 시도.
                if let Some(id) = cur_id.take() {
                    if let std::collections::hash_map::Entry::Vacant(e) = map.entry(id) {
                        if let Some(level) = heading_level(e.key(), e.key()) {
                            e.insert(level);
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => return Err(ParseError::markup(FMT, format!("styles.xml: {e}"))),
        }
    }
    Ok(map)
}

/// docProps/core.xml → title/author.
fn parse_core_props(pkg: &OoxmlPackage, meta: &mut DocumentMetadata) -> Result<(), ParseError> {
    let xml = match pkg.get_str("docProps/core.xml", FMT)? {
        Some(x) => x,
        None => return Ok(()),
    };
    let mut reader = Reader::from_str(&xml);
    let mut cur: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                cur = match e.name().as_ref() {
                    b"dc:title" => Some("title"),
                    b"dc:creator" => Some("author"),
                    _ => None,
                };
            }
            Ok(Event::Text(t)) => {
                if let Some(field) = cur {
                    let val = t
                        .unescape()
                        .map_err(|e| ParseError::markup(FMT, format!("core.xml: {e}")))?
                        .trim()
                        .to_string();
                    if !val.is_empty() {
                        match field {
                            "title" => meta.title = Some(val),
                            "author" => meta.author = Some(val),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(_)) => cur = None,
            Ok(_) => {}
            Err(e) => return Err(ParseError::markup(FMT, format!("core.xml: {e}"))),
        }
    }
    Ok(())
}

/// 표 빌드 컨텍스트 (중첩 표 지원용 스택 요소).
struct TableCtx {
    rows: Vec<Vec<String>>,
    cur_row: Vec<String>,
    cell_buf: String,
}

impl TableCtx {
    fn new() -> Self {
        TableCtx { rows: Vec::new(), cur_row: Vec::new(), cell_buf: String::new() }
    }
    fn into_table(self) -> Table {
        let mut rows = self.rows;
        let headers = if rows.is_empty() { Vec::new() } else { rows.remove(0) };
        Table { headers, rows, caption: None }
    }
}

/// document.xml 본문 → 블록 목록.
fn parse_body(
    xml: &str,
    styles: &HashMap<String, u8>,
    images: &HashMap<String, (String, ImageData)>,
) -> Result<Vec<Block>, ParseError> {
    let mut reader = Reader::from_str(xml);

    let mut blocks: Vec<Block> = Vec::new();
    let mut pending_list: Vec<ListItem> = Vec::new();
    let mut in_drawing = false; // <w:drawing> 안인지 (그림 blip 판별용)

    // 현재 단락 상태.
    let mut runs: Vec<Inline> = Vec::new();
    let mut para_style: Option<String> = None;
    let mut is_list_item = false;

    // 현재 run 서식.
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut in_rpr = false;
    let mut in_text = false;

    // 표 스택.
    let mut tables: Vec<TableCtx> = Vec::new();

    macro_rules! flush_list {
        () => {
            if !pending_list.is_empty() {
                blocks.push(Block::List { ordered: false, items: std::mem::take(&mut pending_list) });
            }
        };
    }

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ParseError::markup(FMT, format!("document.xml: {e}")))?;
        match ev {
            Event::Eof => break,

            Event::Start(e) => match e.name().as_ref() {
                b"w:p" => {
                    runs.clear();
                    para_style = None;
                    is_list_item = false;
                }
                b"w:r" => {
                    bold = false;
                    italic = false;
                    strike = false;
                }
                b"w:rPr" => in_rpr = true,
                b"w:t" => in_text = true,
                b"w:pStyle" => {
                    if let Some(v) = attr_val(&e, b"w:val") {
                        para_style = Some(v);
                    }
                }
                b"w:numPr" => is_list_item = true,
                b"w:tbl" => tables.push(TableCtx::new()),
                b"w:drawing" | b"w:pict" => in_drawing = true,
                // 그림이 자식을 갖는 경우 blip 이 Start 로 올 수 있다.
                b"a:blip" => {
                    emit_image(&mut blocks, &mut pending_list, &e, in_drawing, &tables, images)
                }
                b"w:tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell_buf.clear();
                    }
                }
                b"w:b" if in_rpr => bold = !is_off(&e),
                b"w:i" if in_rpr => italic = !is_off(&e),
                b"w:strike" if in_rpr => strike = !is_off(&e),
                _ => {}
            },

            Event::Empty(e) => match e.name().as_ref() {
                b"w:pStyle" => {
                    if let Some(v) = attr_val(&e, b"w:val") {
                        para_style = Some(v);
                    }
                }
                b"w:numPr" => is_list_item = true,
                b"w:b" if in_rpr => bold = !is_off(&e),
                b"w:i" if in_rpr => italic = !is_off(&e),
                b"w:strike" if in_rpr => strike = !is_off(&e),
                b"w:br" => push_text(&mut tables, &mut runs, None, false, false, false, true),
                b"w:tab" => push_text(&mut tables, &mut runs, Some("\t"), false, false, false, false),
                b"a:blip" => {
                    emit_image(&mut blocks, &mut pending_list, &e, in_drawing, &tables, images)
                }
                _ => {}
            },

            Event::Text(t) => {
                if in_text {
                    let text = t
                        .unescape()
                        .map_err(|e| ParseError::markup(FMT, format!("w:t: {e}")))?
                        .to_string();
                    push_text(&mut tables, &mut runs, Some(&text), bold, italic, strike, false);
                }
            }

            Event::End(e) => match e.name().as_ref() {
                b"w:rPr" => in_rpr = false,
                b"w:drawing" | b"w:pict" => in_drawing = false,
                b"w:t" => in_text = false,
                b"w:tc" => {
                    if let Some(t) = tables.last_mut() {
                        let cell = collapse(&std::mem::take(&mut t.cell_buf));
                        t.cur_row.push(cell);
                    }
                }
                b"w:tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.rows.push(std::mem::take(&mut t.cur_row));
                    }
                }
                b"w:tbl" => {
                    if let Some(ctx) = tables.pop() {
                        let table = ctx.into_table();
                        if let Some(parent) = tables.last_mut() {
                            // 중첩 표 → 부모 셀에 평문으로 흡수 (v0.1 단순화).
                            for row in table.rows.iter() {
                                parent.cell_buf.push_str(&row.join(" "));
                                parent.cell_buf.push(' ');
                            }
                        } else {
                            flush_list!();
                            blocks.push(Block::Table(table));
                        }
                    }
                }
                b"w:p" => {
                    // 표 안의 단락이면 셀 버퍼에만 누적되므로 별도 블록 생성 안 함.
                    if tables.is_empty() {
                        emit_paragraph(
                            &mut blocks,
                            &mut pending_list,
                            std::mem::take(&mut runs),
                            &para_style,
                            is_list_item,
                            styles,
                        );
                    } else if let Some(t) = tables.last_mut() {
                        // 셀 안 여러 단락 사이 공백 구분.
                        if !t.cell_buf.is_empty() && !t.cell_buf.ends_with(' ') {
                            t.cell_buf.push(' ');
                        }
                    }
                }
                _ => {}
            },

            _ => {}
        }
    }

    flush_list!();
    Ok(blocks)
}

/// 텍스트 조각을 표 셀(평문) 또는 단락 runs(서식 보존)로 라우팅.
#[allow(clippy::too_many_arguments)]
fn push_text(
    tables: &mut [TableCtx],
    runs: &mut Vec<Inline>,
    text: Option<&str>,
    bold: bool,
    italic: bool,
    strike: bool,
    line_break: bool,
) {
    if let Some(t) = tables.last_mut() {
        if line_break {
            t.cell_buf.push(' ');
        } else if let Some(s) = text {
            t.cell_buf.push_str(s);
        }
        return;
    }
    if line_break {
        runs.push(Inline::LineBreak);
        return;
    }
    let Some(s) = text else { return };
    let mut inl = Inline::Text(s.to_string());
    if strike {
        inl = Inline::Strike(Box::new(inl));
    }
    if italic {
        inl = Inline::Italic(Box::new(inl));
    }
    if bold {
        inl = Inline::Bold(Box::new(inl));
    }
    runs.push(inl);
}

/// 단락 종료 → Heading/List/Paragraph 블록 emit.
fn emit_paragraph(
    blocks: &mut Vec<Block>,
    pending_list: &mut Vec<ListItem>,
    runs: Vec<Inline>,
    para_style: &Option<String>,
    is_list_item: bool,
    styles: &HashMap<String, u8>,
) {
    let plain = plain_text(&runs);

    // 헤딩 판정: styles 맵 우선, 없으면 styleId 자체 휴리스틱.
    let level = para_style.as_ref().and_then(|s| {
        styles.get(s).copied().or_else(|| heading_level(s, s))
    });

    if let Some(level) = level {
        flush_pending(blocks, pending_list);
        if !plain.trim().is_empty() {
            blocks.push(Block::Heading { level, text: plain });
        }
        return;
    }

    if is_list_item {
        if !runs.is_empty() {
            pending_list.push(ListItem::new(runs));
        }
        return;
    }

    flush_pending(blocks, pending_list);
    if !plain.trim().is_empty() {
        blocks.push(Block::Paragraph(runs));
    }
}

fn flush_pending(blocks: &mut Vec<Block>, pending_list: &mut Vec<ListItem>) {
    if !pending_list.is_empty() {
        blocks.push(Block::List { ordered: false, items: std::mem::take(pending_list) });
    }
}

/// `<a:blip r:embed>` → 그림(`w:drawing`) 안이고 표 밖이면 `Block::Image` 추가.
/// 표 셀 안 이미지는 v0.1 에서 생략(셀은 평문만).
fn emit_image(
    blocks: &mut Vec<Block>,
    pending_list: &mut Vec<ListItem>,
    e: &quick_xml::events::BytesStart,
    in_drawing: bool,
    tables: &[TableCtx],
    images: &HashMap<String, (String, ImageData)>,
) {
    if !in_drawing || !tables.is_empty() {
        return;
    }
    if let Some(rid) = attr_val(e, b"r:embed") {
        if let Some((alt, data)) = images.get(&rid) {
            flush_pending(blocks, pending_list);
            blocks.push(Block::Image { alt: alt.clone(), data: data.clone() });
        }
    }
}

// ── 유틸 ─────────────────────────────────────────────────────

/// `<w:b w:val="false"/>` / `"0"` → 끔. 그 외(빈 태그 포함) → 켬.
fn is_off(e: &quick_xml::events::BytesStart) -> bool {
    matches!(attr_val(e, b"w:val").as_deref(), Some("false") | Some("0") | Some("none"))
}

/// 시작/빈 태그에서 속성 값 추출.
fn attr_val(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == key {
            a.unescape_value().ok().map(|v| v.to_string())
        } else {
            None
        }
    })
}

/// Inline 트리에서 평문만 추출.
fn plain_text(runs: &[Inline]) -> String {
    let mut s = String::new();
    for r in runs {
        collect_plain(r, &mut s);
    }
    s
}

fn collect_plain(inl: &Inline, out: &mut String) {
    match inl {
        Inline::Text(t) | Inline::Code(t) => out.push_str(t),
        Inline::Bold(i) | Inline::Italic(i) | Inline::Strike(i) => collect_plain(i, out),
        Inline::Link { text, .. } => out.push_str(text),
        Inline::LineBreak => out.push(' '),
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// styleId/스타일명에서 Heading 레벨 추론.
fn heading_level(style_id: &str, style_name: &str) -> Option<u8> {
    let id = style_id.to_ascii_lowercase();
    let name = style_name.to_ascii_lowercase();
    for s in [name.as_str(), id.as_str()] {
        if s == "title" {
            return Some(1);
        }
        if s == "subtitle" {
            return Some(2);
        }
        if let Some(rest) = s.strip_prefix("heading") {
            let n = rest.trim_matches(|c: char| !c.is_ascii_digit());
            return Some(n.parse::<u8>().unwrap_or(1).clamp(1, 6));
        }
    }
    None
}
