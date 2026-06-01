//! # rust_markdown_transformer
//!
//! 만능 문서 → Markdown 변환 플러그인 라이브러리. docx · pptx · xlsx · html · markdown
//! (그리고 향후 hwp/hwpx/pdf/epub/…) 을 **벡터 DB 친화적 Markdown** 으로 결정적이고 빠르게 변환한다.
//!
//! ## 설계 (README 요약)
//! - **Deterministic** — 같은 입력 → 같은 출력.
//! - **Structure-preserving** — 제목 계층·표·목록·코드블록을 Markdown 문법으로 충실히 재현.
//! - **Plugin-extensible** — 새 포맷은 [`FormatParser`] 하나만 구현하면 끝 (코어 불변).
//! - **Zero-dependency self-contained** — default 빌드는 pure Rust / zero FFI.
//!
//! ## 파이프라인
//! ```text
//! 구체 파서(feature) → FormatParser → IR(Document) → MarkdownRenderer → (Markdown)
//!                                          └→ SemanticChunker → Vec<Chunk> → Vector DB
//! ```
//!
//! ## 사용 예시
//! ```no_run
//! use rust_markdown_transformer::{ParserRegistry, SemanticChunker};
//!
//! let registry = ParserRegistry::with_defaults();
//! let md = registry.convert_to_markdown("report.docx".as_ref())?;
//!
//! let doc = registry.parse_to_ir("report.docx".as_ref())?;
//! let chunks = SemanticChunker::default().chunk(&doc);
//! # Ok::<(), rust_markdown_transformer::ConvertError>(())
//! ```

pub mod chunker;
pub mod error;
pub mod ir;
pub mod parsers;
pub mod registry;
pub mod renderer;

#[cfg(feature = "python")]
mod python;

// ── 자주 쓰는 타입 re-export ──────────────────────────────────
pub use chunker::{Chunk, HeuristicTokenCounter, SemanticChunker, TokenCounter};
pub use error::{ConvertError, ParseError};
pub use ir::{
    Block, Document, DocumentMetadata, ImageData, Inline, ListItem, NestedList, SourceFormat, Table,
};
pub use registry::{FormatParser, ParserRegistry};
pub use renderer::MarkdownRenderer;
