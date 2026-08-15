//! # rust_pii_transformer
//!
//! 한국어 텍스트에서 개인정보를 **결정적으로** 탐지하고, **원문 복원이 보장되는** 마스킹을
//! 수행하는 **모델 없는 순수 Rust** 라이브러리.
//!
//! 모델 파일 0개, GPU 0, 외부 API 호출 0, 네트워크 접근 0. 폐쇄망에서 그대로 돈다.
//!
//! ## 핵심 차별점은 변형 표기 인식이다
//!
//! 순수 숫자열뿐 아니라 한글 수사 표기(`팔팔공일공일`), 띄어쓰기와 구분자 변형
//! (`880101 - 1234567`), 한글과 숫자 혼합 표기(`88년 1월 1일생`)를 잡는다.
//!
//! ## 4층 파이프라인
//!
//! ```text
//! 원문 → [정규화] → (정규화문, SpanMap) → [탐지] → Finding(정규화 좌표)
//!                          └────────────────────────→ Finding(원문 좌표) → [마스킹]
//! ```
//!
//! 탐지는 깨끗한 정규화문 위에서 하고 마스킹은 원문 위에서 한다. 둘을 잇는 것이
//! [`SpanMap`] 하나뿐이라 복원 정확성의 책임이 한 곳에 모인다.
//!
//! ## 현재 구현 상태
//!
//! **네 층이 모두 동작한다.** 합성 검증 코퍼스 3,160건 기준 재현율 99.9퍼센트, 정밀도
//! 99.3퍼센트이고 마스킹 원문 복원 실패는 0건이다. 씨앗이 고정이라 누구든 같은 수를 얻는다.
//!
//! ```bash
//! cargo test --test accuracy -- --nocapture
//! ```
//!
//! 이 수치는 합성 코퍼스에서 잰 값이고 실제 한국어 문서 표본으로 잰 값이 아니다. 회귀를 막고
//! 변경의 영향을 보는 데는 충분하지만, 자기 데이터에서 다시 재고 임계값을 맞추는 것을 권한다.
//! 남은 한계는 저장소 `README.md` 의 알려진 한계 절에 적는다.
//!
//! | 모듈 | 상태 |
//! | --- | --- |
//! | [`span`] | 동작 확인. `SpanMap`, `Segment`, 합성, 역방향 매핑, 불변식 검사 |
//! | [`normalize`] | 동작 확인. 5개 패스(자모 조합, 전각 폴딩, 한글 수사, 유사문자 교정, 구분자 흡수) |
//! | [`detect`] | 동작 확인. 검증식, 스캐너, 문맥 점수, 판정 근거와 미탐 사유 |
//! | [`mask`] | 동작 확인. 정책 4종(전체 치환·부분 노출·해시·토큰화)과 복원 맵 |
//! | [`synth`] | 동작 확인. 검증식이 유효한 표본과 표기 변형 생성 |
//! | `python` | 동작 확인. 네 층 전부의 PyO3 바인딩 (`--features python`) |
//!
//! 해시 가명화 정책은 `hash`, 명령줄 도구 `rpit` 은 `cli` 기능 플래그를 켰을 때만 들어온다.
//!
//! ## 탐지
//!
//! ```
//! use rust_pii_transformer::detect::{detect, Config};
//!
//! let text = "주민등록번호 팔팔공일공일 - 1234567 로 접수했습니다";
//! let report = detect(text, &Config::default()).unwrap();
//!
//! // 한글 수사와 구분자 변형을 정규화 층이 흡수한 뒤 잡는다.
//! assert_eq!(report.findings.len(), 1);
//! let found = &report.findings[0];
//! assert_eq!(found.source.slice(text), "팔팔공일공일 - 1234567");
//!
//! // 왜 걸렸는지가 함께 실린다.
//! assert_eq!(found.evidence.rule, "resident.korean_13");
//! assert!(found.evidence.cost.expanded_syllables > 0);
//! ```
//!
//! ## 마스킹
//!
//! ```
//! use rust_pii_transformer::detect::Config;
//! use rust_pii_transformer::{mask, unmask, Policy, PolicySet};
//!
//! let text = "연락처 010-1234-5678 입니다";
//!
//! // 토큰화 정책은 원문 복원이 보장된다.
//! let out = mask(text, &Config::default(), &PolicySet::new(Policy::Tokenize)).unwrap();
//! assert_ne!(out.text, text);
//!
//! let restore = out.restore.as_ref().unwrap();
//! assert_eq!(unmask(&out.text, restore).unwrap(), text);
//! ```
//!
//! ## 오프셋 매핑을 직접 쓰기
//!
//! 자기 정규화 패스를 만들고 그 결과를 원문 좌표로 되돌리고 싶을 때 쓴다.
//!
//! ```
//! use rust_pii_transformer::{Absorbed, Span, SpanMapBuilder};
//!
//! // "880101-1234567" 에서 하이픈을 흡수한다.
//! let mut builder = SpanMapBuilder::new();
//! builder.keep("880101");
//! builder.absorb("-", "separator.hyphen", Absorbed::Separator);
//! builder.keep("1234567");
//! let (normalized, map) = builder.finish();
//! assert_eq!(normalized, "8801011234567");
//!
//! // 정규화문에서 찾은 스팬을 원문 좌표로 되돌린다.
//! let found = Span::new(0..13, 0..13);
//! let source = map.to_source(&found);
//! assert_eq!(source.span.byte, 0..14);          // 원문에서는 하이픈까지 14바이트
//! assert_eq!(source.cost.absorbed_separators, 1); // 판정 근거로 실린다
//! ```

pub mod detect;
pub mod error;
pub mod mask;
pub mod normalize;
pub mod span;
pub mod synth;

/// PyO3 바인딩. `--features python` 일 때만 빌드된다.
#[cfg(feature = "python")]
pub mod python;

pub use error::{Error, Result};
pub use mask::{mask, unmask, MaskOutput, Policy, PolicySet, Redaction, RestoreMap};
pub use normalize::{normalize, NormalizeConfig, Normalized, NumeralConfig};
pub use span::{
    Absorbed, NormalizationCost, RuleId, Segment, SegmentKind, SourceSpan, Span, SpanMap,
    SpanMapBuilder,
};
