//! 타입드 에러와 검색 결과 보고. 공개 라이브러리 API 는 anyhow 가 아니라 타입드 에러를
//! 노출해 호출자가 실패 종류로 분기할 수 있게 한다.

use crate::model::{Document, SourceKind};
use serde::{Deserialize, Serialize};

/// 소스 어댑터·수집 단계에서 발생하는 에러.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("http request failed: {0}")]
    Http(String),
    #[error("parse failed: {0}")]
    Parse(String),
    #[error("rate limit exceeded: {0}")]
    RateLimit(String),
    #[error("invalid query: {0}")]
    InvalidQuery(String),
}

/// 한 소스의 부분 실패. 전체 검색을 실패시키지 않고 결과와 함께 보고된다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorWarning {
    pub source: SourceKind,
    pub message: String,
}

impl ConnectorWarning {
    pub fn new(source: SourceKind, message: impl Into<String>) -> Self {
        Self { source, message: message.into() }
    }

    pub fn timeout(source: SourceKind) -> Self {
        Self { source, message: "source timed out".to_string() }
    }

    /// 소스가 정상 응답했지만 결과가 0건인 경우. 실패가 아니라 정보지만 경고로 올린다 —
    /// 이게 없으면 호출자가 "이 소스에 없었다"와 "이 소스가 안 돌았다"를 구분할 수 없다.
    pub fn empty(source: SourceKind) -> Self {
        Self { source, message: "source returned 0 documents".to_string() }
    }
}

/// 검색 결과 + 부분 실패 경고. 부분 실패를 조용히 버리지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReport {
    pub docs: Vec<Document>,
    pub warnings: Vec<ConnectorWarning>,
}
