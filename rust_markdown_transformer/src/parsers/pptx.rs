//! PPTX 파서 — README §5-1 (PPTX 분기).
//!
//! - `ppt/slides/slideN.xml` 각 슬라이드를 순서대로 처리하고 슬라이드 사이에 `Block::PageBreak`.
//! - `<p:ph type="title"|"ctrTitle">` 플레이스홀더 텍스트 → `Block::Heading { level: 2 }`.
//! - 본문 텍스트박스(`<a:p>/<a:r>/<a:t>`) → `Block::Paragraph`.
//! - `<a:rPr b="1" i="1">` → Bold/Italic 보존.

use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::ParseError;
use crate::ir::*;
use crate::parsers::ooxml::OoxmlPackage;
use crate::registry::FormatParser;

const FMT: &str = "pptx";

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
            parse_slide(&xml, &mut blocks)?;
        }

        Ok(Document { metadata, blocks })
    }
}

fn parse_slide(xml: &str, blocks: &mut Vec<Block>) -> Result<(), ParseError> {
    let mut reader = Reader::from_str(xml);

    let mut is_title_shape = false; // 현재 shape 가 title placeholder 인지
    let mut runs: Vec<Inline> = Vec::new(); // 현재 <a:p> 의 runs
    let mut bold = false;
    let mut italic = false;
    let mut in_text = false;

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ParseError::markup(FMT, format!("slide: {e}")))?;
        match ev {
            Event::Eof => break,

            Event::Start(e) => match e.name().as_ref() {
                b"p:sp" => is_title_shape = false,
                b"a:p" => runs.clear(),
                b"a:rPr" => {
                    bold = is_on(&e, b"b");
                    italic = is_on(&e, b"i");
                }
                b"a:t" => in_text = true,
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
                b"a:br" => runs.push(Inline::LineBreak),
                _ => {}
            },

            Event::Text(t) => {
                if in_text {
                    let s = t
                        .unescape()
                        .map_err(|e| ParseError::markup(FMT, format!("a:t: {e}")))?
                        .to_string();
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

            Event::End(e) => match e.name().as_ref() {
                b"a:t" => in_text = false,
                b"a:r" => {
                    bold = false;
                    italic = false;
                }
                b"a:p" => {
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
                _ => {}
            },

            _ => {}
        }
    }
    Ok(())
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
