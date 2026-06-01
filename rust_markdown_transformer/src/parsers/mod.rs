//! 포맷별 구체 파서들. 각 파서는 feature flag 로 게이트되며 [`FormatParser`](crate::FormatParser)
//! 를 구현한다. v0.1: docx / pptx / xlsx / html / markdown (모두 pure Rust, zero FFI).

#[cfg(any(feature = "docx", feature = "pptx", feature = "hwpx"))]
pub(crate) mod ooxml;

#[cfg(feature = "docx")]
mod docx;
#[cfg(feature = "docx")]
pub use docx::DocxParser;

#[cfg(feature = "pptx")]
mod pptx;
#[cfg(feature = "pptx")]
pub use pptx::PptxParser;

#[cfg(feature = "xlsx")]
mod xlsx;
#[cfg(feature = "xlsx")]
pub use xlsx::XlsxParser;

#[cfg(feature = "hwpx")]
mod hwpx;
#[cfg(feature = "hwpx")]
pub use hwpx::HwpxParser;

#[cfg(feature = "pdf")]
mod pdf;
#[cfg(feature = "pdf")]
pub use pdf::PdfParser;

#[cfg(feature = "html")]
mod html;
#[cfg(feature = "html")]
pub use html::HtmlParser;

#[cfg(feature = "markdown")]
mod markdown;
#[cfg(feature = "markdown")]
pub use markdown::MarkdownParser;
