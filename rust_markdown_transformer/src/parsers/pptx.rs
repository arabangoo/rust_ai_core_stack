//! PPTX 파서 — README §5-1 (PPTX 분기).
//!
//! - `ppt/slides/slideN.xml` 각 슬라이드를 순서대로 처리하고 슬라이드 사이에 `Block::PageBreak`.
//! - `<p:ph type="title"|"ctrTitle">` 플레이스홀더 텍스트 → `Block::Heading { level: 2 }`.
//! - 본문 텍스트박스(`<a:p>/<a:r>/<a:t>`) → `Block::Paragraph`.
//! - `<a:rPr b="1" i="1">` → Bold/Italic 보존.
//! - DrawingML 표(`<a:tbl>/<a:tr>/<a:tc>`) → `Block::Table` (셀은 평문).
//! - 그림(`<p:pic>/<a:blip r:embed>`) → 슬라이드 관계(.rels)로 media 파트를 찾아 `Block::Image`.

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::ParseError;
use crate::ir::*;
use crate::parsers::ooxml::OoxmlPackage;
use crate::registry::FormatParser;

const FMT: &str = "pptx";

/// rId → (alt stem, 이미지 데이터).
type ImageMap = HashMap<String, (String, ImageData)>;

pub struct PptxParser;

impl FormatParser for PptxParser {
    fn supported_extensions(&self) -> &[&str] {
        &["pptx"]
    }

    fn name(&self) -> &'static str {
        "pptx"
    }

    fn can_parse_bytes(&self, header: &[u8]) -> bool {
        header.starts_with(&[0x50, 0x4B, 0x03, 0x04])
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let pkg = OoxmlPackage::from_reader(input, FMT)?;

        let slides = pkg.names_matching("ppt/slides/slide", ".xml");
        let mut metadata = DocumentMetadata::new(SourceFormat::Pptx, filename);
        metadata.page_count = Some(slides.len());

        let mut blocks: Vec<Block> = Vec::new();
        for (i, slide) in slides.iter().enumerate() {
            if i > 0 {
                blocks.push(Block::PageBreak);
            }
            let xml = pkg
                .get_str(slide, FMT)?
                .ok_or_else(|| ParseError::container(FMT, format!("missing {slide}")))?;
            // 슬라이드별 그림 관계 (`ppt/slides/_rels/slideN.xml.rels`).
            let images = pkg.image_rels(slide, FMT)?;
            parse_slide(&xml, &mut blocks, &images)?;
        }

        Ok(Document { metadata, blocks })
    }
}

/// 표 빌드 컨텍스트 (DrawingML `<a:tbl>`).
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

fn parse_slide(xml: &str, blocks: &mut Vec<Block>, images: &ImageMap) -> Result<(), ParseError> {
    let mut reader = Reader::from_str(xml);

    let mut is_title_shape = false; // 현재 shape 가 title placeholder 인지
    let mut runs: Vec<Inline> = Vec::new(); // 현재 <a:p> 의 runs (텍스트박스용)
    let mut bold = false;
    let mut italic = false;
    let mut in_text = false;
    let mut in_pic = false; // 현재 <p:pic>(그림) 안인지
    let mut tables: Vec<TableCtx> = Vec::new(); // 표 스택

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ParseError::markup(FMT, format!("slide: {e}")))?;
        match ev {
            Event::Eof => break,

            Event::Start(e) => match e.name().as_ref() {
                b"p:sp" => is_title_shape = false,
                b"p:pic" => in_pic = true,
                b"a:tbl" => tables.push(TableCtx::new()),
                b"a:tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.cur_row.clear();
                    }
                }
                b"a:tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell_buf.clear();
                    }
                }
                b"a:p" => runs.clear(),
                b"a:rPr" => {
                    bold = is_on(&e, b"b");
                    italic = is_on(&e, b"i");
                }
                b"a:t" => in_text = true,
                // 그림이 자식(extLst 등)을 갖는 드문 경우 blip 이 Start 로 올 수 있다.
                b"a:blip" => push_image(blocks, &e, in_pic, &tables, images),
                _ => {}
            },

            Event::Empty(e) => match e.name().as_ref() {
                b"p:ph" => {
                    if let Some(t) = attr_val(&e, b"type") {
                        if t == "title" || t == "ctrTitle" {
                            is_title_shape = true;
                        }
                    }
                }
                b"a:rPr" => {
                    bold = is_on(&e, b"b");
                    italic = is_on(&e, b"i");
                }
                b"a:br" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell_buf.push(' ');
                    } else {
                        runs.push(Inline::LineBreak);
                    }
                }
                b"a:blip" => push_image(blocks, &e, in_pic, &tables, images),
                _ => {}
            },

            Event::Text(t) => {
                if in_text {
                    let s = t
                        .unescape()
                        .map_err(|e| ParseError::markup(FMT, format!("a:t: {e}")))?
                        .to_string();
                    if let Some(tbl) = tables.last_mut() {
                        // 표 셀 안 → 평문 누적.
                        tbl.cell_buf.push_str(&s);
                    } else {
                        let mut inl = Inline::Text(s);
                        if italic {
                            inl = Inline::Italic(Box::new(inl));
                        }
                        if bold {
                            inl = Inline::Bold(Box::new(inl));
                        }
                        runs.push(inl);
                    }
                }
            }

            Event::End(e) => match e.name().as_ref() {
                b"a:t" => in_text = false,
                b"a:r" => {
                    bold = false;
                    italic = false;
                }
                b"p:pic" => in_pic = false,
                b"a:tc" => {
                    if let Some(t) = tables.last_mut() {
                        let cell = collapse(&std::mem::take(&mut t.cell_buf));
                        t.cur_row.push(cell);
                    }
                }
                b"a:tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.rows.push(std::mem::take(&mut t.cur_row));
                    }
                }
                b"a:tbl" => {
                    if let Some(ctx) = tables.pop() {
                        blocks.push(Block::Table(ctx.into_table()));
                    }
                }
                b"a:p" => {
                    if let Some(t) = tables.last_mut() {
                        // 셀 안 여러 단락 사이 공백 구분.
                        if !t.cell_buf.is_empty() && !t.cell_buf.ends_with(' ') {
                            t.cell_buf.push(' ');
                        }
                    } else {
                        let plain: String = plain_text(&runs);
                        if !plain.trim().is_empty() {
                            if is_title_shape {
                                blocks.push(Block::Heading { level: 2, text: plain });
                            } else {
                                blocks.push(Block::Paragraph(std::mem::take(&mut runs)));
                            }
                        }
                        runs.clear();
                    }
                }
                _ => {}
            },

            _ => {}
        }
    }
    Ok(())
}

/// `<a:blip r:embed>` → 그림(`p:pic`) 안이고 표 밖이면 `Block::Image` 추가.
fn push_image(
    blocks: &mut Vec<Block>,
    e: &quick_xml::events::BytesStart,
    in_pic: bool,
    tables: &[TableCtx],
    images: &ImageMap,
) {
    if !in_pic || !tables.is_empty() {
        return;
    }
    if let Some(rid) = attr_val(e, b"r:embed") {
        if let Some((alt, data)) = images.get(&rid) {
            blocks.push(Block::Image { alt: alt.clone(), data: data.clone() });
        }
    }
}

/// DrawingML 불리언 속성 (`b="1"`, `i="1"`, `b="true"`).
fn is_on(e: &quick_xml::events::BytesStart, key: &[u8]) -> bool {
    matches!(attr_val(e, key).as_deref(), Some("1") | Some("true"))
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

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plain_text(runs: &[Inline]) -> String {
    let mut s = String::new();
    for r in runs {
        match r {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Bold(i) | Inline::Italic(i) | Inline::Strike(i) => {
                if let Inline::Text(t) = &**i {
                    s.push_str(t);
                }
            }
            Inline::Link { text, .. } => s.push_str(text),
            Inline::LineBreak => s.push(' '),
        }
    }
    s
}
