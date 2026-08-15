//! 크레이트 전역 에러 타입.
//!
//! 독립성 원칙을 지키기 위해 에러 타입은 optional 의존성에 의존하지 않는다.
//! 어떤 feature 조합에서도 항상 컴파일된다.

use thiserror::Error;

/// 이 크레이트의 표준 결과 타입.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// 크레이트 전역 단일 에러 열거형.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// 구간 정렬 테이블의 불변식이 깨졌다.
    ///
    /// 사용자 입력으로는 발생하지 않는다. 정규화 패스 구현 버그를 뜻하므로
    /// 발생하면 그 패스를 고쳐야 한다.
    #[error("span map invariant violated at segment {index}: {detail}")]
    SpanMapInvariant { index: usize, detail: String },

    /// 두 구간 정렬 테이블의 좌표계가 맞지 않는다.
    ///
    /// `SpanMap::compose` 에서 앞 테이블의 출력 텍스트와 뒤 테이블의 입력 텍스트가
    /// 다를 때 발생한다. 파이프라인 조립 순서가 틀렸다는 신호다.
    #[error("span map coordinate mismatch: {detail}")]
    SpanMapMismatch { detail: String },

    /// 마스킹 복원 중 토큰을 해석하지 못했다.
    ///
    /// 마스킹 텍스트가 편집됐거나 복원 맵이 다른 문서의 것일 때 발생한다.
    /// 조용히 넘기지 않는 이유는, 복원 실패를 눈치채지 못한 채 "원문을 되찾았다"고 믿는 것이
    /// 이 라이브러리가 막으려는 바로 그 사고이기 때문이다.
    #[error("restore token error: {detail}")]
    RestoreToken { detail: String },
}

impl Error {
    pub(crate) fn invariant(index: usize, detail: impl Into<String>) -> Self {
        Error::SpanMapInvariant { index, detail: detail.into() }
    }

    pub(crate) fn mismatch(detail: impl Into<String>) -> Self {
        Error::SpanMapMismatch { detail: detail.into() }
    }

    pub(crate) fn restore_token(detail: impl Into<String>) -> Self {
        Error::RestoreToken { detail: detail.into() }
    }
}
