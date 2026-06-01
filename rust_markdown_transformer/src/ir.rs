//! 공통 IR(Internal Representation) — README §3.
//!
//! 모든 구체 파서는 포맷 고유 구조를 이 포맷 중립 IR 로 변환하고,
//! [`MarkdownRenderer`](crate::MarkdownRenderer) 와 [`SemanticChunker`](crate::SemanticChunker)
//! 는 오직 이 IR 만 본다 → 파서와 렌더러/청커가 완전히 분리된다.
//!
//! IR 은 `serde` 직렬화를 지원하므로 그대로 `*.ir.json` (Dual-track 안전망, README §1-3)
//! 으로 떨어뜨릴 수 있다.

use serde::{Deserialize, Serialize};

/// 문서를 포맷 중립적으로 표현하는 최상위 IR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub metadata: DocumentMetadata,
    pub blocks: Vec<Block>,
}

impl Document {
    /// 메타데이터만 주고 빈 블록으로 시작.
    pub fn new(metadata: DocumentMetadata) -> Self {
        Document { metadata, blocks: Vec::new() }
    }

    /// 블록을 push 하는 빌더 스타일 헬퍼.
    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }
}

/// 문서 메타데이터. Markdown frontmatter / 벡터 DB 메타데이터로 직결된다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub source_format: SourceFormat,
    pub original_filename: String,
    pub page_count: Option<usize>,
    pub language: Option<String>,
}

impl DocumentMetadata {
    /// 포맷과 파일명만으로 최소 메타데이터 생성.
    pub fn new(source_format: SourceFormat, original_filename: impl Into<String>) -> Self {
        DocumentMetadata {
            title: None,
            author: None,
            created_at: None,
            source_format,
            original_filename: original_filename.into(),
            page_count: None,
            language: None,
        }
    }
}

/// 원본 포맷 식별자.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    Docx,
    Pptx,
    Xlsx,
    Hwp,
    Hwpx,
    Pdf,
    Html,
    Markdown,
    Epub,
    Rtf,
    Odt,
    Unknown,
}

/// 블록 레벨 요소 (Markdown 표현 범위와 거의 1:1 대응).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// `#`~`######` (level 1~6).
    Heading { level: u8, text: String },
    /// 인라인 요소들로 구성된 단락.
    Paragraph(Vec<Inline>),
    /// 표.
    Table(Table),
    /// 순서/비순서 목록.
    List { ordered: bool, items: Vec<ListItem> },
    /// 펜스 코드 블록.
    CodeBlock { lang: Option<String>, code: String },
    /// 인용구.
    Quote(Vec<Inline>),
    /// 수평선 `---`.
    HorizontalRule,
    /// 이미지 (base64 또는 경로/URL 참조).
    Image { alt: String, data: ImageData },
    /// 수식 (인라인/디스플레이).
    Math { latex: String, display: bool },
    /// 페이지 경계 (PPT 슬라이드 / PDF 페이지).
    PageBreak,
    /// 각주.
    Footnote { id: String, content: Vec<Inline> },
}

/// 인라인 레벨 요소.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Inline {
    Text(String),
    Bold(Box<Inline>),
    Italic(Box<Inline>),
    Strike(Box<Inline>),
    Code(String),
    Link { text: String, url: String },
    LineBreak,
}

impl Inline {
    /// 일반 텍스트 헬퍼.
    pub fn text(s: impl Into<String>) -> Self {
        Inline::Text(s.into())
    }
}

/// 목록 항목. `sublist` 로 중첩 목록을 표현한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub content: Vec<Inline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sublist: Option<Box<NestedList>>,
}

impl ListItem {
    pub fn new(content: Vec<Inline>) -> Self {
        ListItem { content, sublist: None }
    }
}

/// 중첩 목록 (항목 내부에 들어가는 하위 목록).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestedList {
    pub ordered: bool,
    pub items: Vec<ListItem>,
}

/// 표. 병합셀(rowspan/colspan)은 v0.1 에서 미지원 → 첫 셀 값만 보존 (README §12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// 이미지 데이터 표현.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImageData {
    /// base64 인코딩된 인라인 데이터 (`mime` 예: `image/png`).
    Base64 { mime: String, data: String },
    /// 로컬/상대 경로 참조.
    Path(String),
    /// 외부 URL 참조.
    Url(String),
}
