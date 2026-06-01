//! PyO3 바인딩 — README §9. 모든 document_ai 파이프라인의 ingestion 단 어댑터.
//!
//! `feature = "python"` 활성 시 cdylib 로 빌드되어 Python 에서 `import rust_markdown_transformer`
//! 로 바로 사용한다. abi3(stable ABI)로 빌드되어 Python 버전 변화에 forward-compatible.
//!
//! ```python
//! import rust_markdown_transformer as rmt
//! md     = rmt.convert_to_markdown("./report.hwpx")          # 청킹·임베딩용
//! ir     = rmt.convert_to_ir_json("./report.hwpx")           # 멀티모달/citation 안전망
//! chunks = rmt.convert_to_chunks("./report.pdf", 512, 64)    # 벡터 DB 적재용 JSON
//! ok     = rmt.is_supported("./x.docx")
//! ```

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::{ParserRegistry, SemanticChunker};

/// 파일 → Markdown 문자열.
#[pyfunction]
fn convert_to_markdown(path: &str) -> PyResult<String> {
    ParserRegistry::with_defaults()
        .convert_to_markdown(std::path::Path::new(path))
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// 파일 → IR JSON 문자열 (Dual-track 안전망, README §1-3).
#[pyfunction]
fn convert_to_ir_json(path: &str) -> PyResult<String> {
    let doc = ParserRegistry::with_defaults()
        .parse_to_ir(std::path::Path::new(path))
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    serde_json::to_string(&doc).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// 파일 → Semantic Chunk 목록의 JSON 문자열 (벡터 DB 적재용).
#[pyfunction]
#[pyo3(signature = (path, max_tokens=512, overlap=64, heading_levels=vec![1, 2]))]
fn convert_to_chunks(
    path: &str,
    max_tokens: usize,
    overlap: usize,
    heading_levels: Vec<u8>,
) -> PyResult<String> {
    let doc = ParserRegistry::with_defaults()
        .parse_to_ir(std::path::Path::new(path))
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    let chunker = SemanticChunker { max_tokens, overlap_tokens: overlap, heading_levels };
    let chunks = chunker.chunk(&doc);
    serde_json::to_string(&chunks).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// 확장자/매직바이트로 지원 여부 판별.
#[pyfunction]
fn is_supported(path: &str) -> bool {
    ParserRegistry::with_defaults().is_supported(std::path::Path::new(path))
}

/// 등록된 파서 이름 목록 (활성 feature 기준).
#[pyfunction]
fn supported_parsers() -> Vec<String> {
    ParserRegistry::with_defaults()
        .parser_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

/// Python 모듈 정의 — 모듈명은 cdylib 이름(`rust_markdown_transformer`)과 일치해야 한다.
#[pymodule]
fn rust_markdown_transformer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(convert_to_markdown, m)?)?;
    m.add_function(wrap_pyfunction!(convert_to_ir_json, m)?)?;
    m.add_function(wrap_pyfunction!(convert_to_chunks, m)?)?;
    m.add_function(wrap_pyfunction!(is_supported, m)?)?;
    m.add_function(wrap_pyfunction!(supported_parsers, m)?)?;
    Ok(())
}
