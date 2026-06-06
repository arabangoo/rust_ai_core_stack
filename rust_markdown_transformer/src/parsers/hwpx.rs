//! HWPX 파서 — README §5-2 (전략 1: HWPX 우선).
//!
//! HWPX(한컴 OWPML)는 ZIP+XML 이라 DOCX 와 거의 같은 방식으로 다룬다.
//!
//! - `Contents/header.xml` 의 `<hh:style id engName="Outline N">` → **styleID → Heading 레벨** 맵.
//!   (한글 `name` 은 인코딩 이슈가 있으나 `engName` 은 ASCII 라 안정적)
//! - `Contents/section{0,1,…}.xml` 의 `<hp:p styleIDRef>/<hp:run>/<hp:t>` → Heading/Paragraph,
//!   `<hp:tbl>/<hp:tr>/<hp:tc>` → Table.
//! - 네임스페이스 prefix 변동에 견디도록 **local name**(`:` 뒤)으로 비교한다.
//!
//! 구조/엘리먼트명은 공개 OWPML 스펙 및 MIT 라이선스 `rhwp` 구조 학습 기반 clean-room 재구현.
//! 글자모양(굵게/기울임)은 charPr 참조라 v0.2 에서는 미보존(텍스트·헤딩·표 우선).

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::ParseError;
use crate::ir::*;
use crate::parsers::ooxml::OoxmlPackage;
use crate::registry::FormatParser;

const FMT: &str = "hwpx";

pub struct HwpxParser;

impl FormatParser for HwpxParser {
    fn supported_extensions(&self) -> &[&str] {
        &["hwpx"]
    }

    fn name(&self) -> &'static str {
        "hwpx"
    }

    fn can_parse_bytes(&self, _header: &[u8]) -> bool {
        // ZIP 매직(PK)은 OOXML 과 공통이라 매직 폴백으로는 식별 불가 → 확장자 디스패치만.
        false
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let pkg = OoxmlPackage::from_reader(input, FMT)?;

        let styles = parse_header_styles(&pkg)?;

        let sections = pkg.names_matching("Contents/section", ".xml");
        if sections.is_empty() {
            return Err(ParseError::container(FMT, "no Contents/section*.xml"));
        }

        let mut metadata = DocumentMetadata::new(SourceFormat::Hwpx, filename);
        metadata.title = parse_title(&pkg);

        // content.hpf 매니페스트 → binaryItemID → 이미지.
        let images = build_images(&pkg)?;

        let mut blocks: Vec<Block> = Vec::new();
        for section in &sections {
            let xml = pkg
                .get_str(section, FMT)?
                .ok_or_else(|| ParseError::container(FMT, format!("missing {section}")))?;
            parse_section(&xml, &styles, &images, &mut blocks)?;
        }

        Ok(Document { metadata, blocks })
    }
}

/// header.xml → styleID → Heading 레벨 맵 (engName "Outline N"/"Title" 기준).
fn parse_header_styles(pkg: &OoxmlPackage) -> Result<HashMap<String, u8>, ParseError> {
    let mut map = HashMap::new();
    let xml = match pkg.get_str("Contents/header.xml", FMT)? {
        Some(x) => x,
        None => return Ok(map),
    };
    let mut reader = Reader::from_str(&xml);
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local_name(e.name().as_ref()) == b"style" => {
                let id = attr_val(&e, b"id");
                let eng = attr_val(&e, b"engName").unwrap_or_default();
                if let (Some(id), Some(level)) = (id, heading_level(&eng)) {
                    map.insert(id, level);
                }
            }
            Ok(_) => {}
            Err(e) => return Err(ParseError::markup(FMT, format!("header.xml: {e}"))),
        }
    }
    Ok(map)
}

/// content.hpf(opf) 의 `<dc:title>` 추출 (있으면).
fn parse_title(pkg: &OoxmlPackage) -> Option<String> {
    let xml = pkg.get_str("Contents/content.hpf", FMT).ok().flatten()?;
    let mut reader = Reader::from_str(&xml);
    let mut in_title = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == b"title" => in_title = true,
            Ok(Event::Text(t)) if in_title => {
                let s = t.unescape().ok()?.trim().to_string();
                return if s.is_empty() { None } else { Some(s) };
            }
            Ok(Event::End(_)) => in_title = false,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    None
}

/// content.hpf 매니페스트(`<opf:item id href media-type>`)로 BinData 이미지를 적재.
/// → binaryItemID → (alt stem, base64 [`ImageData`]). 이미지(media-type `image/*`)만.
fn build_images(pkg: &OoxmlPackage) -> Result<HashMap<String, (String, ImageData)>, ParseError> {
    let mut out = HashMap::new();
    let xml = match pkg.get_str("Contents/content.hpf", FMT)? {
        Some(x) => x,
        None => return Ok(out),
    };
    let mut reader = Reader::from_str(&xml);
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if local_name(e.name().as_ref()) == b"item" =>
            {
                let id = attr_val(&e, b"id");
                let href = attr_val(&e, b"href");
                let media = attr_val(&e, b"media-type").unwrap_or_default();
                if let (Some(id), Some(href)) = (id, href) {
                    let path_mime = super::media::mime_from_path(&href);
                    if !media.starts_with("image/") && !path_mime.starts_with("image/") {
                        continue;
                    }
                    // href 가 패키지 루트/Contents 중 어느 기준이든 폴백 해석.
                    if let Some(bytes) = pkg.get(&href).or_else(|| pkg.get_by_suffix(&href)) {
                        let mime = if media.starts_with("image/") {
                            media.clone()
                        } else {
                            path_mime.to_string()
                        };
                        let data = ImageData::Base64 {
                            mime,
                            data: super::media::base64_encode(bytes),
                        };
                        out.insert(id, (super::media::stem_of(&href), data));
                    }
                }
            }
            Ok(_) => {}
            Err(e) => return Err(ParseError::markup(FMT, format!("content.hpf: {e}"))),
        }
    }
    Ok(out)
}

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

type ImageMap = HashMap<String, (String, ImageData)>;

fn parse_section(
    xml: &str,
    styles: &HashMap<String, u8>,
    images: &ImageMap,
    blocks: &mut Vec<Block>,
) -> Result<(), ParseError> {
    let mut reader = Reader::from_str(xml);

    let mut para_text = String::new();
    let mut para_style: Option<String> = None;
    let mut in_text = false;
    let mut tables: Vec<TableCtx> = Vec::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ParseError::markup(FMT, format!("section: {e}")))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                // 그림: BinData 를 참조하는 binaryItemIDRef 속성을 가진 엘리먼트
                // (`<hp:pic>/<hc:img binaryItemIDRef>`). 표 밖에서만 블록으로.
                if tables.is_empty() {
                    if let Some(rid) = attr_val_local(&e, b"binaryItemIDRef") {
                        if let Some((alt, data)) = images.get(&rid) {
                            blocks.push(Block::Image { alt: alt.clone(), data: data.clone() });
                        }
                    }
                }
                match local_name(e.name().as_ref()) {
                    b"p" => {
                        para_text.clear();
                        para_style = attr_val(&e, b"styleIDRef");
                    }
                    b"t" => in_text = true,
                    b"tbl" => tables.push(TableCtx::new()),
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            t.cell_buf.clear();
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_text {
                    let s = t
                        .unescape()
                        .map_err(|e| ParseError::markup(FMT, format!("hp:t: {e}")))?;
                    if let Some(tbl) = tables.last_mut() {
                        tbl.cell_buf.push_str(&s);
                    } else {
                        para_text.push_str(&s);
                    }
                }
            }
            Event::End(e) => match local_name(e.name().as_ref()) {
                b"t" => in_text = false,
                b"p" => {
                    if let Some(tbl) = tables.last_mut() {
                        if !tbl.cell_buf.is_empty() && !tbl.cell_buf.ends_with(' ') {
                            tbl.cell_buf.push(' ');
                        }
                    } else {
                        emit_paragraph(blocks, &para_text, &para_style, styles);
                    }
                }
                b"tc" => {
                    if let Some(tbl) = tables.last_mut() {
                        let cell = collapse(&std::mem::take(&mut tbl.cell_buf));
                        tbl.cur_row.push(cell);
                    }
                }
                b"tr" => {
                    if let Some(tbl) = tables.last_mut() {
                        tbl.rows.push(std::mem::take(&mut tbl.cur_row));
                    }
                }
                b"tbl" => {
                    if let Some(ctx) = tables.pop() {
                        let table = ctx.into_table();
                        if let Some(parent) = tables.last_mut() {
                            for row in table.rows.iter() {
                                parent.cell_buf.push_str(&row.join(" "));
                                parent.cell_buf.push(' ');
                            }
                        } else {
                            blocks.push(Block::Table(table));
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

fn emit_paragraph(
    blocks: &mut Vec<Block>,
    text: &str,
    para_style: &Option<String>,
    styles: &HashMap<String, u8>,
) {
    let text = collapse(text);
    if text.is_empty() {
        return;
    }
    let level = para_style.as_ref().and_then(|s| styles.get(s).copied());
    match level {
        Some(level) => blocks.push(Block::Heading { level, text }),
        None => blocks.push(Block::Paragraph(vec![Inline::Text(text)])),
    }
}

// ── 유틸 ─────────────────────────────────────────────────────

/// `b"hp:p"` → `b"p"`, `b"tbl"` → `b"tbl"` (네임스페이스 prefix 제거).
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr_val(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == key {
            a.unescape_value().ok().map(|v| v.to_string())
        } else {
            None
        }
    })
}

/// 속성 키를 **local name**(`:` 뒤)으로 비교 (네임스페이스 prefix 변동 대비).
fn attr_val_local(e: &quick_xml::events::BytesStart, local: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if local_name(a.key.as_ref()) == local {
            a.unescape_value().ok().map(|v| v.to_string())
        } else {
            None
        }
    })
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// engName → Heading 레벨. "Outline N"/"Heading N" → N(1~6), "Title"→1, "Subtitle"→2.
fn heading_level(eng_name: &str) -> Option<u8> {
    let s = eng_name.trim().to_ascii_lowercase();
    if s == "title" {
        return Some(1);
    }
    if s == "subtitle" {
        return Some(2);
    }
    for prefix in ["outline", "heading"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let n = rest.trim().parse::<u8>().unwrap_or(1);
            return Some(n.clamp(1, 6));
        }
    }
    None
}
