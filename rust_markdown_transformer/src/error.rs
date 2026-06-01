//! 크레이트 전역 에러 타입.
//!
//! 독립성 원칙(README §1-4)을 지키기 위해 에러 타입은 **optional 의존성에 의존하지 않는다**.
//! `zip` / `quick-xml` / `calamine` 등의 구체 에러는 각 파서에서 문자열로 변환해
//! [`ParseError`] 의 문자열 variant 로 흡수한다 → feature 조합과 무관하게 항상 컴파일된다.

use thiserror::Error;

/// 개별 파서가 IR 변환 중 낼 수 있는 에러.
#[derive(Debug, Error)]
pub enum ParseError {
    /// 입력 스트림 I/O 실패.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// 컨테이너(zip/OLE2 등) 구조가 손상됐거나 기대한 엔트리가 없음.
    #[error("malformed {format} container: {detail}")]
    Container {
        format: &'static str,
        detail: String,
    },

    /// XML/마크업 파싱 실패.
    #[error("{format} markup parse error: {detail}")]
    Markup {
        format: &'static str,
        detail: String,
    },

    /// 텍스트 인코딩 디코딩 실패 (EUC-KR/CP949/UTF-16 등).
    #[error("text encoding error in {format}: {detail}")]
    Encoding {
        format: &'static str,
        detail: String,
    },

    /// 해당 파서가 아직 지원하지 않는 입력 형태.
    #[error("unsupported in {format}: {detail}")]
    Unsupported {
        format: &'static str,
        detail: String,
    },
}

impl ParseError {
    /// 컨테이너 에러 헬퍼.
    pub fn container(format: &'static str, detail: impl Into<String>) -> Self {
        ParseError::Container { format, detail: detail.into() }
    }

    /// 마크업 에러 헬퍼.
    pub fn markup(format: &'static str, detail: impl Into<String>) -> Self {
        ParseError::Markup { format, detail: detail.into() }
    }

    /// 인코딩 에러 헬퍼.
    pub fn encoding(format: &'static str, detail: impl Into<String>) -> Self {
        ParseError::Encoding { format, detail: detail.into() }
    }
}

/// [`ParserRegistry`](crate::ParserRegistry) 의 상위 변환 API 가 내는 에러.
#[derive(Debug, Error)]
pub enum ConvertError {
    /// 파일 열기/읽기 실패.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// 등록된 어떤 파서도 이 확장자/매직바이트를 처리하지 못함.
    #[error("unsupported format: '{0}' (no registered parser)")]
    UnsupportedFormat(String),

    /// 매칭된 파서가 변환 도중 실패.
    #[error(transparent)]
    Parse(#[from] ParseError),
}
