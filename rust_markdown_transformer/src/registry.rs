//! 플러그인 트레이트 + 런타임 레지스트리 — README §4.
//!
//! 새 포맷을 추가하려면 [`FormatParser`] 하나만 구현해 [`ParserRegistry::register`] 로 끼우면 된다.
//! 코어(IR/렌더러/청커)는 전혀 건드리지 않는다 → 포맷 추가가 O(1) 에 가깝다 (README §1-2 Plugin-extensible).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{ConvertError, ParseError};
use crate::ir::Document;
use crate::renderer::MarkdownRenderer;

/// 하나의 문서 포맷을 IR 로 변환하는 플러그인.
pub trait FormatParser: Send + Sync {
    /// 이 파서가 처리하는 확장자 목록 (소문자, 점 없이 — 예: `["docx"]`).
    fn supported_extensions(&self) -> &[&str];

    /// 매직 바이트 기반 식별 (확장자 없음 / 위조 대비).
    fn can_parse_bytes(&self, header: &[u8]) -> bool;

    /// 사람이 읽는 파서 이름 (로그/디버깅용).
    fn name(&self) -> &'static str;

    /// 스트리밍 입력 → IR.
    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError>;
}

/// 등록된 파서들을 보관하고 입력을 적절한 파서로 디스패치하는 레지스트리.
#[derive(Default)]
pub struct ParserRegistry {
    parsers: Vec<Box<dyn FormatParser>>,
}

impl ParserRegistry {
    /// 빈 레지스트리.
    pub fn empty() -> Self {
        ParserRegistry { parsers: Vec::new() }
    }

    /// 활성화된 feature 의 기본 파서들을 모두 등록한 레지스트리.
    pub fn with_defaults() -> Self {
        let mut r = ParserRegistry::empty();
        #[cfg(feature = "docx")]
        r.register(Box::new(crate::parsers::DocxParser));
        #[cfg(feature = "pptx")]
        r.register(Box::new(crate::parsers::PptxParser));
        #[cfg(feature = "xlsx")]
        r.register(Box::new(crate::parsers::XlsxParser));
        #[cfg(feature = "hwpx")]
        r.register(Box::new(crate::parsers::HwpxParser));
        #[cfg(feature = "pdf")]
        r.register(Box::new(crate::parsers::PdfParser));
        #[cfg(feature = "html")]
        r.register(Box::new(crate::parsers::HtmlParser));
        #[cfg(feature = "markdown")]
        r.register(Box::new(crate::parsers::MarkdownParser));
        r
    }

    /// 서드파티/커스텀 파서 등록.
    pub fn register(&mut self, parser: Box<dyn FormatParser>) {
        self.parsers.push(parser);
    }

    /// 등록된 파서 이름 목록.
    pub fn parser_names(&self) -> Vec<&'static str> {
        self.parsers.iter().map(|p| p.name()).collect()
    }

    /// 확장자/매직바이트로 이 입력을 처리할 수 있는지 여부.
    pub fn is_supported(&self, path: &Path) -> bool {
        let ext = extension_of(path);
        if self.find_by_ext(&ext).is_some() {
            return true;
        }
        // 매직바이트 폴백.
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut header = [0u8; 16];
            if f.read(&mut header).is_ok() {
                return self.find_by_magic(&header).is_some();
            }
        }
        false
    }

    /// 파일 경로 → IR. 확장자 우선, 실패 시 매직바이트로 파서를 고른다.
    pub fn parse_to_ir(&self, path: &Path) -> Result<Document, ConvertError> {
        let ext = extension_of(path);
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let mut file = std::fs::File::open(path)?;
        let mut header = [0u8; 16];
        let n = file.read(&mut header)?;
        file.seek(SeekFrom::Start(0))?;

        let parser = self
            .find_by_ext(&ext)
            .or_else(|| self.find_by_magic(&header[..n]))
            .ok_or_else(|| ConvertError::UnsupportedFormat(ext.clone()))?;

        Ok(parser.parse(&mut file, &filename)?)
    }

    /// 파일 경로 → Markdown 문자열.
    pub fn convert_to_markdown(&self, path: &Path) -> Result<String, ConvertError> {
        let doc = self.parse_to_ir(path)?;
        Ok(MarkdownRenderer::render(&doc))
    }

    /// 임의 reader → IR. 확장자를 모를 때 `ext_hint` (예: stdin 파이프 `--from pdf`) 를 준다.
    /// reader 전체를 메모리로 읽어 매직바이트 판별 + Seek 가능 커서로 파서에 넘긴다.
    pub fn parse_reader(
        &self,
        reader: &mut dyn Read,
        filename: &str,
        ext_hint: Option<&str>,
    ) -> Result<Document, ConvertError> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;

        let parser = ext_hint
            .and_then(|e| self.find_by_ext(&e.to_ascii_lowercase()))
            .or_else(|| self.find_by_magic(&buf))
            .ok_or_else(|| ConvertError::UnsupportedFormat(ext_hint.unwrap_or("").to_string()))?;

        let mut cursor = std::io::Cursor::new(buf);
        Ok(parser.parse(&mut cursor, filename)?)
    }

    fn find_by_ext(&self, ext: &str) -> Option<&dyn FormatParser> {
        if ext.is_empty() {
            return None;
        }
        self.parsers
            .iter()
            .find(|p| p.supported_extensions().contains(&ext))
            .map(|b| b.as_ref())
    }

    fn find_by_magic(&self, header: &[u8]) -> Option<&dyn FormatParser> {
        self.parsers
            .iter()
            .find(|p| p.can_parse_bytes(header))
            .map(|b| b.as_ref())
    }
}

/// 경로에서 소문자 확장자 추출 (점 제외).
fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
