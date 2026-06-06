//! PDF 파서 — README §5-3 (기본 빌드: pure Rust, zero FFI).
//!
//! PDF 는 본질적으로 시각적 레이아웃 포맷이라 의미 구조 복원이 어렵다. v0.2 기본 파서는:
//! - **텍스트**: `pdf-extract`(lopdf 기반, MIT) 로 추출 — 폰트 인코딩/ToUnicode CMap 을
//!   처리해 **한글 CID 폰트도 올바른 Unicode** 로 복원한다. (텍스트 정확도 > 헤딩 휴리스틱)
//! - **메타데이터**: `lopdf` 로 /Info 의 Title/Author + 페이지 수.
//! - **구조**: form-feed(`\u{0C}`) 페이지 경계 → `Block::PageBreak`, 빈 줄 → 단락 분리.
//!
//! 폰트 크기 기반 **헤딩 복원 + reading-order(XY-Cut++)** 는 README 로드맵대로 **v0.4** 로 이연한다.
//! 스캔 PDF(텍스트 레이어 없음)는 빈 문서로 반환한다(알림 후 skip — OCR 은 `feature = "ocr"` opt-in).
//!
//! ⚠️ 알고리즘 출처(§1-5): 텍스트 배치/reading-order 는 poppler(GPL) 코드가 아니라
//! ISO 32000-1 텍스트 모델 + 공개 논문 기반 clean-room. poppler/MuPDF 미열람.

use std::io::Read;

use crate::error::ParseError;
use crate::ir::*;
use crate::registry::FormatParser;

const FMT: &str = "pdf";

pub struct PdfParser;

impl FormatParser for PdfParser {
    fn supported_extensions(&self) -> &[&str] {
        &["pdf"]
    }

    fn name(&self) -> &'static str {
        "pdf"
    }

    fn can_parse_bytes(&self, header: &[u8]) -> bool {
        // PDF 매직: "%PDF-"
        header.starts_with(b"%PDF-")
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;

        let mut metadata = DocumentMetadata::new(SourceFormat::Pdf, filename);

        // doc 를 한 번 로드해 메타데이터·layout·이미지 추출에 재사용.
        let doc = lopdf::Document::load_mem(&buf).ok();

        // 1차: layout 레이어 (좌표·폰트크기 기반 헤딩 + XY-Cut 읽기순서).
        if let Some(doc) = &doc {
            fill_metadata(doc, &mut metadata);
            let mut blocks = super::pdf_layout::layout_blocks(doc);
            if !blocks.is_empty() {
                blocks.extend(extract_images(doc));
                return Ok(Document { metadata, blocks });
            }
        }

        // 2차(fallback): 텍스트 레이어가 없거나 layout 실패 → 단순 텍스트 추출.
        // pdf-extract 가 일부 손상 PDF 에서 panic 할 수 있어 격리한다 (README §1-1).
        let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&buf))
            .map_err(|_| ParseError::container(FMT, "pdf-extract panicked on malformed PDF"))?
            .map_err(|e| ParseError::container(FMT, format!("text extraction failed: {e}")))?;

        let mut blocks = text_to_blocks(&text);
        if let Some(doc) = &doc {
            blocks.extend(extract_images(doc));
        }
        Ok(Document { metadata, blocks })
    }
}

/// lopdf Document 에서 /Info(Title/Author) + 페이지 수 추출 (best-effort).
fn fill_metadata(doc: &lopdf::Document, meta: &mut DocumentMetadata) {
    meta.page_count = Some(doc.get_pages().len());
    meta.title = info_field(doc, b"Title");
    meta.author = info_field(doc, b"Author");
}

/// 페이지 순서대로 내장 이미지를 추출해 `Block::Image` 목록으로 반환.
/// 본문 텍스트 뒤에 모아 붙인다(정확한 본문 내 위치 매핑은 콘텐츠 스트림 분석 필요 — 후속 과제).
/// lopdf 호출이 손상 PDF 에서 panic 할 가능성을 격리한다 (README §1-1).
fn extract_images(doc: &lopdf::Document) -> Vec<Block> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collect_images(doc)))
        .unwrap_or_default()
}

fn collect_images(doc: &lopdf::Document) -> Vec<Block> {
    let mut out = Vec::new();
    for (_num, page_id) in doc.get_pages() {
        let images = match doc.get_page_images(page_id) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for img in images {
            if let Some(mime) = supported_image_mime(&img) {
                out.push(Block::Image {
                    alt: format!("image-{}-{}", img.id.0, img.id.1),
                    data: ImageData::Base64 {
                        mime: mime.to_string(),
                        data: super::media::base64_encode(img.content),
                    },
                });
            }
        }
    }
    out
}

/// 스트림 바이트가 **그대로 완전한 이미지 파일**인 필터만 MIME 으로 매핑.
/// DCTDecode = JPEG, JPXDecode = JPEG2000. FlateDecode/무압축 raster(샘플 재구성 필요)는
/// width/height/colorspace 로 PNG 를 합성하는 후속 과제로 남긴다.
fn supported_image_mime(img: &lopdf::xobject::PdfImage<'_>) -> Option<&'static str> {
    let filters = img.filters.as_ref()?;
    if filters.iter().any(|f| f == "DCTDecode") {
        Some("image/jpeg")
    } else if filters.iter().any(|f| f == "JPXDecode") {
        Some("image/jp2")
    } else {
        None
    }
}

fn info_field(doc: &lopdf::Document, key: &[u8]) -> Option<String> {
    let info_obj = doc.trailer.get(b"Info").ok()?;
    let info = match info_obj {
        lopdf::Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    let dict = info.as_dict().ok()?;
    if let lopdf::Object::String(bytes, _) = dict.get(key).ok()? {
        let s = decode_pdf_text(bytes);
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// PDF 문자열 디코딩: UTF-16BE(BOM FE FF) 또는 PDFDocEncoding(~Latin1).
fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16s: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| ((c[0] as u16) << 8) | (*c.get(1).unwrap_or(&0) as u16))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        // PDFDocEncoding 은 ASCII 영역에서 Latin1 과 동일.
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// 추출된 평문 → IR 블록. form-feed 로 페이지 분리, 빈 줄로 단락 분리.
fn text_to_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let pages: Vec<&str> = text.split('\u{0C}').collect();

    for (pi, page) in pages.iter().enumerate() {
        if pi > 0 {
            blocks.push(Block::PageBreak);
        }
        for para in split_paragraphs(page) {
            blocks.push(Block::Paragraph(vec![Inline::Text(para)]));
        }
    }
    blocks
}

/// 빈 줄 경계로 단락 분리. 단락 내부의 단일 개행은 공백으로 접는다.
fn split_paragraphs(page: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let mut cur: Vec<&str> = Vec::new();

    let flush = |cur: &mut Vec<&str>, paras: &mut Vec<String>| {
        if !cur.is_empty() {
            let joined = cur.join(" ");
            let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
            if !collapsed.is_empty() {
                paras.push(collapsed);
            }
            cur.clear();
        }
    };

    for line in page.lines() {
        if line.trim().is_empty() {
            flush(&mut cur, &mut paras);
        } else {
            cur.push(line);
        }
    }
    flush(&mut cur, &mut paras);
    paras
}
