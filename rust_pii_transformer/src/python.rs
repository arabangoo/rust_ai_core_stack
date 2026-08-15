//! PyO3 바인딩 — README 15절.
//!
//! `feature = "python"` 을 켰을 때만 빌드된다. abi3(안정 이진 인터페이스) 확장 모듈이라
//! Python 3.9 이상에서 휠 하나로 돈다.
//!
//! ## 코어를 오염시키지 않는다
//!
//! [`crate::span`] 의 타입에 `#[pyclass]` 를 직접 달지 않고 이 모듈 안에 래퍼를 둔다.
//! 그래야 기본 빌드가 PyO3 를 전혀 모르는 순수 Rust 로 남는다(독립성 원칙, README 16절).
//!
//! ## 노출 범위는 지금 실제로 도는 것까지다
//!
//! 네 층(`span`, `normalize`, `detect`, `mask`)을 모두 낸다. 껍데기는 만들지 않는다.
//! 모듈 수준 `__status__` 로 현재 범위를 밝힌다.
//!
//! ```python
//! import rust_pii_transformer as rpit
//!
//! # 탐지. 한글 수사로 적힌 주민등록번호도 원문 좌표로 되돌아온다.
//! report = rpit.detect("주민등록번호 팔팔공일공일 - 1234567 입니다")
//! f = report.findings[0]
//! f.entity, f.certainty          # ('resident', 'probable')
//! f.source.char_start            # 문자 좌표가 그대로 나온다
//!
//! # 마스킹. 토큰화는 원문 복원이 보장된다.
//! out = rpit.mask("연락처 010-1234-5678", rpit.PolicySet(rpit.Policy.tokenize()))
//! rpit.unmask(out.text, out.restore) == "연락처 010-1234-5678"
//! ```
//!
//! ## 열거형은 문자열로 낸다
//!
//! `EntityKind` 나 `Certainty` 를 `#[pyclass]` 열거형으로 만들지 않고 소문자 문자열로 낸다.
//! Python 쪽에서 비교와 직렬화가 그대로 되고, 사전 키로 바로 쓸 수 있기 때문이다.
//! 사람이 읽는 한국어 이름이 필요하면 `entity_label` 을 따로 쓴다.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModule;
use pyo3::wrap_pyfunction;

use crate::detect::{
    detect as core_detect, Certainty, ChecksumResult, Config, ContextHit, EntityKind, Evidence,
    Finding, RejectReason, Rejection, Report,
};
use crate::mask::{
    mask as core_mask, unmask as core_unmask, MaskOutput, Policy, PolicySet, Redaction,
    RestoreEntry, RestoreMap,
};
use crate::normalize::normalize as core_normalize;
use crate::span::{
    Absorbed, NormalizationCost, RuleId, Segment, SegmentKind, SourceSpan, Span, SpanMap,
    SpanMapBuilder,
};

// ── 예외 ────────────────────────────────────────────────────

create_exception!(
    rust_pii_transformer,
    SpanMapError,
    PyException,
    "구간 정렬 테이블의 불변식이 깨졌거나 좌표계가 맞지 않는다."
);

fn map_err(e: crate::Error) -> PyErr {
    SpanMapError::new_err(e.to_string())
}

// ── 규칙 식별자 인터닝 ───────────────────────────────────────

/// Python 이 넘긴 규칙 이름을 `&'static str` 로 바꾼다.
///
/// 코어는 규칙 이름을 `&'static str` 로 들고 다녀 힙 할당을 0 으로 만든다(README 6절).
/// 그 선택이 Python 경계에서는 대가를 만든다. 런타임에 들어온 문자열에는 `'static` 수명이
/// 없으므로 누출(leak)로 승격시켜야 한다.
///
/// 같은 이름은 한 번만 누출되도록 풀에 담는다. 규칙 이름은 설계상 정규화 패스가 가진
/// 고정된 소수 집합이므로 실사용에서 풀 크기는 상수에 수렴한다.
///
/// **한계를 감추지 않는다.** 호출자가 매번 새로운 규칙 이름을 만들어 넣으면
/// (예: `f"rule.{i}"` 를 반복) 그만큼 메모리가 누적되고 회수되지 않는다.
/// 규칙 이름은 상수로 두고 값은 다른 인자로 넘기는 것이 올바른 사용법이다.
fn intern(name: &str) -> RuleId {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    // 잠금이 오염돼도 풀 자체는 여전히 정합하므로 그대로 이어 쓴다.
    let mut guard = pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(found) = guard.get(name) {
        return found;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

// ── 문자열 변환 ─────────────────────────────────────────────

fn kind_name(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Identity => "identity",
        SegmentKind::Replace => "replace",
        SegmentKind::Delete => "delete",
        SegmentKind::Expand => "expand",
    }
}

fn parse_absorbed(class: &str) -> PyResult<Absorbed> {
    match class {
        "whitespace" => Ok(Absorbed::Whitespace),
        "separator" => Ok(Absorbed::Separator),
        "other" => Ok(Absorbed::Other),
        _ => Err(PyValueError::new_err(format!(
            "unknown absorbed class {class:?}; expected 'whitespace', 'separator', or 'other'"
        ))),
    }
}

/// Python 이 보는 엔티티 이름. 코어의 `code()` 를 그대로 쓴다.
///
/// 여기서 표를 따로 두지 않는 이유가 있다. 한때 Python 속성은 `"resident"` 를,
/// JSON 은 `"Resident"` 를 내서 같은 값에 이름이 둘 생겼다. 이름을 정하는 곳은 하나여야 한다.
fn entity_name(entity: EntityKind) -> &'static str {
    entity.code()
}

fn parse_entity(name: &str) -> PyResult<EntityKind> {
    EntityKind::from_code(name).ok_or_else(|| {
        let known: Vec<&str> = crate::detect::entity::ALL.iter().map(|k| k.code()).collect();
        PyValueError::new_err(format!(
            "unknown entity {name:?}; expected one of {}",
            known.join(", ")
        ))
    })
}

fn certainty_name(certainty: Certainty) -> &'static str {
    match certainty {
        Certainty::Possible => "possible",
        Certainty::Probable => "probable",
        Certainty::Certain => "certain",
    }
}

fn reason_name(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::ChecksumFailed => "checksum_failed",
        RejectReason::NoContext => "no_context",
        RejectReason::BelowThreshold => "below_threshold",
        RejectReason::Outranked => "outranked",
        RejectReason::BusinessContext => "business_context",
    }
}

fn checksum_name(result: &ChecksumResult) -> &'static str {
    match result {
        ChecksumResult::Passed => "passed",
        ChecksumResult::Failed => "failed",
        ChecksumResult::NotApplicable(_) => "not_applicable",
    }
}

/// 검증식이 적용 불가일 때의 사유. 나머지 경우엔 `None`.
fn checksum_reason(result: &ChecksumResult) -> Option<&'static str> {
    match result {
        ChecksumResult::NotApplicable(why) => Some(why),
        _ => None,
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

// ── Span ────────────────────────────────────────────────────

/// 텍스트 한 구간. 바이트 오프셋과 문자 오프셋을 둘 다 담는다.
///
/// Python 문자열은 문자 인덱스 기반이라 문자 좌표를 그대로 노출하는 것이 중요하다.
/// 바이트만 주면 소비자 쪽에서 다시 변환해야 하고 그 변환이 새 오프셋 버그의 출처가 된다.
#[pyclass(name = "Span", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PySpan {
    inner: Span,
}

#[pymethods]
impl PySpan {
    #[new]
    fn new(byte_start: u32, byte_end: u32, char_start: u32, char_end: u32) -> Self {
        Self { inner: Span::new(byte_start..byte_end, char_start..char_end) }
    }

    /// 문자 구간만 알 때 텍스트를 훑어 바이트 구간까지 채운 스팬을 만든다.
    ///
    /// Python 소비자가 문자 인덱스만 들고 있는 흔한 상황을 위한 편의 생성자다.
    #[staticmethod]
    fn from_char_range(text: &str, char_start: u32, char_end: u32) -> PyResult<Self> {
        if char_start > char_end {
            return Err(PyValueError::new_err(format!(
                "char_start {char_start} must not exceed char_end {char_end}"
            )));
        }
        // bounds[i] = i 번째 문자의 시작 바이트, 마지막 항목은 텍스트 끝.
        let mut bounds: Vec<u32> = text.char_indices().map(|(b, _)| b as u32).collect();
        bounds.push(text.len() as u32);
        let last = (bounds.len() - 1) as u32;
        if char_end > last {
            return Err(PyValueError::new_err(format!(
                "char_end {char_end} is out of range; text has {last} characters"
            )));
        }
        Ok(Self::new(
            bounds[char_start as usize],
            bounds[char_end as usize],
            char_start,
            char_end,
        ))
    }

    #[getter]
    fn byte_start(&self) -> u32 {
        self.inner.byte.start
    }

    #[getter]
    fn byte_end(&self) -> u32 {
        self.inner.byte.end
    }

    #[getter]
    fn char_start(&self) -> u32 {
        self.inner.char.start
    }

    #[getter]
    fn char_end(&self) -> u32 {
        self.inner.char.end
    }

    #[getter]
    fn byte_len(&self) -> u32 {
        self.inner.byte_len()
    }

    #[getter]
    fn char_len(&self) -> u32 {
        self.inner.char_len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 이 스팬이 가리키는 조각을 잘라 낸다. 범위를 벗어나면 예외를 낸다.
    ///
    /// 코어의 `Span::slice` 는 범위 밖에서 패닉하므로, Python 으로 패닉을 넘기지 않도록
    /// 여기서 먼저 확인한다.
    fn slice(&self, text: &str) -> PyResult<String> {
        let (start, end) = (self.inner.byte.start as usize, self.inner.byte.end as usize);
        if end > text.len() || start > end {
            return Err(PyValueError::new_err(format!(
                "span {start}..{end} is out of range for text of {} bytes",
                text.len()
            )));
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(PyValueError::new_err(format!(
                "span {start}..{end} does not fall on UTF-8 character boundaries"
            )));
        }
        Ok(text[start..end].to_owned())
    }

    fn __repr__(&self) -> String {
        format!(
            "Span(byte={}..{}, char={}..{})",
            self.inner.byte.start, self.inner.byte.end, self.inner.char.start, self.inner.char.end
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ── NormalizationCost ───────────────────────────────────────

/// 이 스팬이 원문에서 얼마나 멀어졌는지의 정량 지표. 탐지 층이 신뢰도를 깎는 데 쓴다.
#[pyclass(name = "NormalizationCost", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyNormalizationCost {
    inner: NormalizationCost,
}

#[pymethods]
impl PyNormalizationCost {
    #[getter]
    fn absorbed_whitespace(&self) -> u16 {
        self.inner.absorbed_whitespace
    }

    #[getter]
    fn absorbed_separators(&self) -> u16 {
        self.inner.absorbed_separators
    }

    #[getter]
    fn expanded_syllables(&self) -> u16 {
        self.inner.expanded_syllables
    }

    #[getter]
    fn replaced_chars(&self) -> u16 {
        self.inner.replaced_chars
    }

    fn is_zero(&self) -> bool {
        self.inner.is_zero()
    }

    fn __repr__(&self) -> String {
        format!(
            "NormalizationCost(absorbed_whitespace={}, absorbed_separators={}, expanded_syllables={}, replaced_chars={})",
            self.inner.absorbed_whitespace,
            self.inner.absorbed_separators,
            self.inner.expanded_syllables,
            self.inner.replaced_chars
        )
    }
}

// ── Segment ─────────────────────────────────────────────────

/// 정규화 한 조각. 연속한 항등 구간은 하나로 접혀 있다.
#[pyclass(name = "Segment", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PySegment {
    inner: Segment,
}

#[pymethods]
impl PySegment {
    /// 입력(원문) 쪽 구간.
    #[getter]
    fn src(&self) -> PySpan {
        PySpan { inner: self.inner.src.clone() }
    }

    /// 출력(정규화문) 쪽 구간.
    #[getter]
    fn dst(&self) -> PySpan {
        PySpan { inner: self.inner.dst.clone() }
    }

    /// `"identity"`, `"replace"`, `"delete"`, `"expand"`, `"insert"` 중 하나.
    #[getter]
    fn kind(&self) -> &'static str {
        kind_name(self.inner.kind)
    }

    /// 이 구간을 만든 규칙들. 이름 오름차순이고 항등 구간은 비어 있다.
    #[getter]
    fn rules(&self) -> Vec<String> {
        self.inner.rules.iter().map(|r| (*r).to_owned()).collect()
    }

    #[getter]
    fn cost(&self) -> PyNormalizationCost {
        PyNormalizationCost { inner: self.inner.cost }
    }

    fn __repr__(&self) -> String {
        format!(
            "Segment(kind={:?}, src={}..{}, dst={}..{})",
            kind_name(self.inner.kind),
            self.inner.src.byte.start,
            self.inner.src.byte.end,
            self.inner.dst.byte.start,
            self.inner.dst.byte.end
        )
    }
}

// ── SourceSpan ──────────────────────────────────────────────

/// [`PySpanMap`] 의 `to_source` 결과. 원문 스팬과 그 판정 근거.
#[pyclass(name = "SourceSpan", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PySourceSpan {
    inner: SourceSpan,
}

#[pymethods]
impl PySourceSpan {
    /// 원문 기준 스팬.
    #[getter]
    fn span(&self) -> PySpan {
        PySpan { inner: self.inner.span.clone() }
    }

    #[getter]
    fn byte_start(&self) -> u32 {
        self.inner.span.byte.start
    }

    #[getter]
    fn byte_end(&self) -> u32 {
        self.inner.span.byte.end
    }

    #[getter]
    fn char_start(&self) -> u32 {
        self.inner.span.char.start
    }

    #[getter]
    fn char_end(&self) -> u32 {
        self.inner.span.char.end
    }

    /// 세그먼트 경계 바깥으로 넓혀졌는가.
    #[getter]
    fn snapped(&self) -> bool {
        self.inner.snapped
    }

    /// 이 구간에 적용된 정규화 규칙 목록. 중복이 제거되고 이름 오름차순으로 정렬돼 있다.
    #[getter]
    fn rules(&self) -> Vec<String> {
        self.inner.rules.iter().map(|r| (*r).to_owned()).collect()
    }

    #[getter]
    fn cost(&self) -> PyNormalizationCost {
        PyNormalizationCost { inner: self.inner.cost }
    }

    /// 판정 근거를 JSON 문자열로 낸다.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner).map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    fn __repr__(&self) -> String {
        format!(
            "SourceSpan(byte={}..{}, char={}..{}, snapped={}, rules={:?})",
            self.inner.span.byte.start,
            self.inner.span.byte.end,
            self.inner.span.char.start,
            self.inner.span.char.end,
            self.inner.snapped,
            self.inner.rules
        )
    }
}

// ── SpanMap ─────────────────────────────────────────────────

/// 원문과 정규화문을 잇는 단조 구간 정렬 테이블.
#[pyclass(name = "SpanMap", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PySpanMap {
    inner: SpanMap,
}

#[pymethods]
impl PySpanMap {
    /// 변환이 전혀 없는 항등 매핑.
    #[staticmethod]
    fn identity(text: &str) -> Self {
        Self { inner: SpanMap::identity(text) }
    }

    /// 두 매핑을 하나로 합친다. `inner` 가 A→B, `outer` 가 B→C 면 결과는 A→C 다.
    ///
    /// `inner` 의 출력 좌표계와 `outer` 의 입력 좌표계가 다르면 `SpanMapError` 를 낸다.
    #[staticmethod]
    fn compose(inner: &PySpanMap, outer: &PySpanMap) -> PyResult<Self> {
        SpanMap::compose(&inner.inner, &outer.inner)
            .map(|m| Self { inner: m })
            .map_err(map_err)
    }

    /// 정규화문 구간을 원문 구간으로 되돌린다.
    fn to_source(&self, dst: &PySpan) -> PySourceSpan {
        PySourceSpan { inner: self.inner.to_source(&dst.inner) }
    }

    /// 문자 좌표만으로 되돌린다. 정규화문을 함께 넘겨 바이트 좌표를 채운다.
    ///
    /// `to_source(Span.from_char_range(normalized, start, end))` 의 축약이다.
    fn to_source_from_chars(
        &self,
        normalized: &str,
        char_start: u32,
        char_end: u32,
    ) -> PyResult<PySourceSpan> {
        let dst = PySpan::from_char_range(normalized, char_start, char_end)?;
        Ok(self.to_source(&dst))
    }

    /// 세그먼트 목록.
    #[getter]
    fn segments(&self) -> Vec<PySegment> {
        self.inner.segments().iter().map(|s| PySegment { inner: s.clone() }).collect()
    }

    /// 변환이 하나도 없는가.
    fn is_identity(&self) -> bool {
        self.inner.is_identity()
    }

    /// 원문 끝 좌표 `(byte, char)`.
    #[getter]
    fn src_end(&self) -> (u32, u32) {
        self.inner.src_end()
    }

    /// 정규화문 끝 좌표 `(byte, char)`.
    #[getter]
    fn dst_end(&self) -> (u32, u32) {
        self.inner.dst_end()
    }

    /// 불변식 검사. 깨졌으면 `SpanMapError` 를 낸다.
    fn validate(&self) -> PyResult<()> {
        self.inner.validate().map_err(map_err)
    }

    /// 테이블 전체를 JSON 문자열로 낸다.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner).map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    fn __len__(&self) -> usize {
        self.inner.segments().len()
    }

    fn __repr__(&self) -> String {
        let (sb, _) = self.inner.src_end();
        let (db, _) = self.inner.dst_end();
        format!(
            "SpanMap(segments={}, src_bytes={sb}, dst_bytes={db})",
            self.inner.segments().len()
        )
    }
}

// ── SpanMapBuilder ──────────────────────────────────────────

/// 정규화 패스가 출력 문자열과 [`PySpanMap`] 을 동시에 만드는 빌더.
///
/// 원문 조각을 앞에서부터 빠짐없이 한 번씩 넘겨야 피복 불변식이 성립한다.
///
/// 코어의 `insert` 는 여기에 노출하지 않는다. 현재 정규화 파이프라인이 쓰지 않는 경로이고,
/// 합성 시 불변식과 결합법칙이 깨지는 것이 실측으로 확인됐기 때문이다. 그 경로가 정리되면
/// 여기에 함께 추가한다.
#[pyclass(name = "SpanMapBuilder", module = "rust_pii_transformer")]
pub struct PySpanMapBuilder {
    /// `finish` 가 빌더를 소비하므로 Option 으로 들고 있다가 꺼낸다.
    inner: Option<SpanMapBuilder>,
}

impl PySpanMapBuilder {
    fn get(&mut self) -> PyResult<&mut SpanMapBuilder> {
        self.inner.as_mut().ok_or_else(|| {
            PyRuntimeError::new_err("this builder was already consumed by finish()")
        })
    }
}

#[pymethods]
impl PySpanMapBuilder {
    /// 빈 빌더. `text` 를 주면 출력 버퍼를 그 길이만큼 미리 잡는다.
    #[new]
    #[pyo3(signature = (text=None))]
    fn new(text: Option<&str>) -> Self {
        let builder = match text {
            Some(t) => SpanMapBuilder::with_capacity(t),
            None => SpanMapBuilder::new(),
        };
        Self { inner: Some(builder) }
    }

    /// 원문 조각을 그대로 통과시킨다. 연속 호출은 하나의 항등 세그먼트로 접힌다.
    fn keep(&mut self, text: &str) -> PyResult<()> {
        self.get()?.keep(text);
        Ok(())
    }

    /// 문자 폴딩. 전각을 반각으로 바꾸거나 유사문자를 교정할 때 쓴다.
    fn replace(&mut self, src_text: &str, dst_text: &str, rule: &str) -> PyResult<()> {
        let rule = intern(rule);
        self.get()?.replace(src_text, dst_text, rule);
        Ok(())
    }

    /// 한글 수사 역변환. `팔팔공일공일` 을 `880101` 로 옮길 때 쓴다.
    fn numeral(&mut self, src_text: &str, dst_text: &str, rule: &str) -> PyResult<()> {
        let rule = intern(rule);
        self.get()?.numeral(src_text, dst_text, rule);
        Ok(())
    }

    /// 원문 조각을 흡수(삭제)한다.
    ///
    /// `class_` 는 `"whitespace"`, `"separator"`, `"other"` 중 하나다. 종류마다 신뢰도
    /// 감점 계수가 다르므로 정확히 골라야 한다.
    #[pyo3(signature = (src_text, rule, class_="separator"))]
    fn absorb(&mut self, src_text: &str, rule: &str, class_: &str) -> PyResult<()> {
        let class = parse_absorbed(class_)?;
        let rule = intern(rule);
        self.get()?.absorb(src_text, rule, class);
        Ok(())
    }

    /// 정규화 결과 문자열과 매핑을 `(str, SpanMap)` 으로 낸다. 빌더는 여기서 소비된다.
    fn finish(&mut self) -> PyResult<(String, PySpanMap)> {
        let builder = self.inner.take().ok_or_else(|| {
            PyRuntimeError::new_err("this builder was already consumed by finish()")
        })?;
        let (text, map) = builder.finish();
        Ok((text, PySpanMap { inner: map }))
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            Some(_) => "SpanMapBuilder(active)".to_owned(),
            None => "SpanMapBuilder(consumed)".to_owned(),
        }
    }
}

// ── Config ──────────────────────────────────────────────────

/// 정규화와 탐지 전체를 아우르는 설정.
///
/// 기본값이 곧 권장값이다. 아무것도 바꾸지 않아도 된다.
#[pyclass(name = "Config", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone, Default)]
pub struct PyConfig {
    inner: Config,
}

#[pymethods]
impl PyConfig {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    /// 문맥 단서를 찾을 창 크기(문자). 기본 24.
    #[getter]
    fn context_window(&self) -> u32 {
        self.inner.context_window
    }

    #[setter]
    fn set_context_window(&mut self, value: u32) {
        self.inner.context_window = value;
    }

    /// 이 점수 미만이면 결과로 내지 않는다. 기본 0.5.
    #[getter]
    fn min_score(&self) -> f32 {
        self.inner.min_score
    }

    #[setter]
    fn set_min_score(&mut self, value: f32) {
        self.inner.min_score = value;
    }

    /// 문맥이 필수인 엔티티가 요구하는 최소 문맥 총점. 기본 0.3.
    #[getter]
    fn min_context(&self) -> f32 {
        self.inner.min_context
    }

    #[setter]
    fn set_min_context(&mut self, value: f32) {
        self.inner.min_context = value;
    }

    /// 부정 문맥이 후보를 버릴 수 있게 되는 최소 총점. 기본 0.5.
    #[getter]
    fn min_veto(&self) -> f32 {
        self.inner.min_veto
    }

    #[setter]
    fn set_min_veto(&mut self, value: f32) {
        self.inner.min_veto = value;
    }

    /// 유니코드 조합 패스를 켠다.
    #[getter]
    fn nfc(&self) -> bool {
        self.inner.normalize.nfc
    }

    #[setter]
    fn set_nfc(&mut self, value: bool) {
        self.inner.normalize.nfc = value;
    }

    /// 전각 폴딩 패스를 켠다.
    #[getter]
    fn fold(&self) -> bool {
        self.inner.normalize.fold
    }

    #[setter]
    fn set_fold(&mut self, value: bool) {
        self.inner.normalize.fold = value;
    }

    /// 한글 수사 역변환 패스를 켠다.
    #[getter]
    fn hangul(&self) -> bool {
        self.inner.normalize.hangul
    }

    #[setter]
    fn set_hangul(&mut self, value: bool) {
        self.inner.normalize.hangul = value;
    }

    /// 유사문자 교정 패스를 켠다.
    #[getter]
    fn lookalike(&self) -> bool {
        self.inner.normalize.lookalike
    }

    #[setter]
    fn set_lookalike(&mut self, value: bool) {
        self.inner.normalize.lookalike = value;
    }

    /// 구분자 흡수 패스를 켠다.
    #[getter]
    fn separator(&self) -> bool {
        self.inner.normalize.separator
    }

    #[setter]
    fn set_separator(&mut self, value: bool) {
        self.inner.normalize.separator = value;
    }

    /// 문맥 단서 없이 한글 수사를 변환할 최소 결과 자릿수. 기본 6.
    #[getter]
    fn numeral_min_digits_without_context(&self) -> usize {
        self.inner.normalize.numeral.min_digits_without_context
    }

    #[setter]
    fn set_numeral_min_digits_without_context(&mut self, value: usize) {
        self.inner.normalize.numeral.min_digits_without_context = value;
    }

    /// 문맥 단서가 있을 때의 최소 결과 자릿수. 기본 2.
    #[getter]
    fn numeral_min_digits_with_context(&self) -> usize {
        self.inner.normalize.numeral.min_digits_with_context
    }

    #[setter]
    fn set_numeral_min_digits_with_context(&mut self, value: usize) {
        self.inner.normalize.numeral.min_digits_with_context = value;
    }

    /// 점수 가중치 일곱 개를 사전으로 낸다.
    #[getter]
    fn weights(&self) -> Vec<(&'static str, f32)> {
        let w = &self.inner.weights;
        vec![
            ("checksum_passed", w.checksum_passed),
            ("checksum_failed", w.checksum_failed),
            ("context", w.context),
            ("absorbed_whitespace", w.absorbed_whitespace),
            ("absorbed_separator", w.absorbed_separator),
            ("expanded_syllable", w.expanded_syllable),
            ("replaced_char", w.replaced_char),
        ]
    }

    /// 점수 가중치를 바꾼다. 주지 않은 것은 그대로 둔다.
    ///
    /// 가중치의 뜻은 저장소 README 의 판정 규칙 절에 있다. 함부로 바꾸면 재현율과 정밀도가
    /// 함께 움직이므로, 바꾼 뒤에는 자기 데이터로 다시 재는 것이 맞다.
    #[pyo3(signature = (
        checksum_passed=None,
        checksum_failed=None,
        context=None,
        absorbed_whitespace=None,
        absorbed_separator=None,
        expanded_syllable=None,
        replaced_char=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn set_weights(
        &mut self,
        checksum_passed: Option<f32>,
        checksum_failed: Option<f32>,
        context: Option<f32>,
        absorbed_whitespace: Option<f32>,
        absorbed_separator: Option<f32>,
        expanded_syllable: Option<f32>,
        replaced_char: Option<f32>,
    ) {
        let w = &mut self.inner.weights;
        if let Some(v) = checksum_passed {
            w.checksum_passed = v;
        }
        if let Some(v) = checksum_failed {
            w.checksum_failed = v;
        }
        if let Some(v) = context {
            w.context = v;
        }
        if let Some(v) = absorbed_whitespace {
            w.absorbed_whitespace = v;
        }
        if let Some(v) = absorbed_separator {
            w.absorbed_separator = v;
        }
        if let Some(v) = expanded_syllable {
            w.expanded_syllable = v;
        }
        if let Some(v) = replaced_char {
            w.replaced_char = v;
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "Config(min_score={}, min_context={}, context_window={})",
            self.inner.min_score, self.inner.min_context, self.inner.context_window
        )
    }
}

// ── Normalized ──────────────────────────────────────────────

/// 정규화 결과. 정규화문과 원문을 잇는 매핑을 함께 낸다.
#[pyclass(name = "Normalized", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyNormalized {
    text: String,
    map: SpanMap,
}

#[pymethods]
impl PyNormalized {
    /// 정규화된 텍스트.
    #[getter]
    fn text(&self) -> &str {
        &self.text
    }

    /// 원문과 정규화문을 잇는 구간 정렬 테이블.
    #[getter]
    fn map(&self) -> PySpanMap {
        PySpanMap { inner: self.map.clone() }
    }

    fn __repr__(&self) -> String {
        format!("Normalized(text={:?}, segments={})", self.text, self.map.segments().len())
    }
}

// ── 탐지 결과 ───────────────────────────────────────────────

/// 문맥 단서 하나가 걸린 기록.
#[pyclass(name = "ContextHit", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyContextHit {
    inner: ContextHit,
}

#[pymethods]
impl PyContextHit {
    /// 걸린 단서 낱말.
    #[getter]
    fn cue(&self) -> &'static str {
        self.inner.cue
    }

    /// 스팬으로부터의 거리(문자).
    #[getter]
    fn distance(&self) -> u32 {
        self.inner.distance
    }

    /// 거리 감쇠가 반영된 무게.
    #[getter]
    fn weight(&self) -> f32 {
        self.inner.weight
    }

    fn __repr__(&self) -> String {
        format!(
            "ContextHit(cue={:?}, distance={}, weight={:.3})",
            self.inner.cue, self.inner.distance, self.inner.weight
        )
    }
}

/// 왜 이 후보가 걸렸는가.
#[pyclass(name = "Evidence", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyEvidence {
    inner: Evidence,
}

#[pymethods]
impl PyEvidence {
    /// 걸린 패턴 규칙 이름.
    #[getter]
    fn rule(&self) -> &'static str {
        self.inner.rule
    }

    /// `"passed"`, `"failed"`, `"not_applicable"` 중 하나.
    #[getter]
    fn checksum(&self) -> &'static str {
        checksum_name(&self.inner.checksum)
    }

    /// 검증식이 적용 불가인 사유. 그 외에는 `None`.
    #[getter]
    fn checksum_reason(&self) -> Option<&'static str> {
        checksum_reason(&self.inner.checksum)
    }

    /// 잡힌 문맥 단서들.
    #[getter]
    fn context_hits(&self) -> Vec<PyContextHit> {
        self.inner.context_hits.iter().map(|h| PyContextHit { inner: *h }).collect()
    }

    /// 이 스팬에 적용된 정규화 규칙들. 이름 오름차순이다.
    #[getter]
    fn normalizations(&self) -> Vec<String> {
        self.inner.normalizations.iter().map(|r| (*r).to_owned()).collect()
    }

    /// 정규화 비용. 감점 근거다.
    #[getter]
    fn cost(&self) -> PyNormalizationCost {
        PyNormalizationCost { inner: self.inner.cost }
    }

    /// 세그먼트 경계 바깥으로 넓혀졌는가.
    #[getter]
    fn snapped(&self) -> bool {
        self.inner.snapped
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Evidence(rule={:?}, checksum={:?}, context_hits={})",
            self.inner.rule,
            checksum_name(&self.inner.checksum),
            self.inner.context_hits.len()
        )
    }
}

/// 탐지 결과 한 건.
#[pyclass(name = "Finding", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyFinding {
    inner: Finding,
}

#[pymethods]
impl PyFinding {
    /// 엔티티 이름. `"resident"`, `"phone"` 처럼 소문자 스네이크 케이스다.
    #[getter]
    fn entity(&self) -> &'static str {
        entity_name(self.inner.entity)
    }

    /// 사람이 읽는 한국어 이름. `"주민등록번호"`
    #[getter]
    fn entity_label(&self) -> &'static str {
        self.inner.entity.label()
    }

    /// **원문** 기준 스팬. 마스킹은 이 구간에 적용된다.
    #[getter]
    fn source(&self) -> PySpan {
        PySpan { inner: self.inner.source.clone() }
    }

    /// 정규화문 기준 스팬.
    #[getter]
    fn normalized(&self) -> PySpan {
        PySpan { inner: self.inner.normalized.clone() }
    }

    /// `"possible"`, `"probable"`, `"certain"` 중 하나.
    #[getter]
    fn certainty(&self) -> &'static str {
        certainty_name(self.inner.certainty)
    }

    /// 연속 점수.
    #[getter]
    fn score(&self) -> f32 {
        self.inner.score
    }

    /// 판정 근거.
    #[getter]
    fn evidence(&self) -> PyEvidence {
        PyEvidence { inner: self.inner.evidence.clone() }
    }

    /// 원문에서 이 결과가 가리키는 조각을 잘라 낸다.
    fn text(&self, source_text: &str) -> PyResult<String> {
        PySpan { inner: self.inner.source.clone() }.slice(source_text)
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Finding(entity={:?}, certainty={:?}, score={:.2}, byte={}..{})",
            entity_name(self.inner.entity),
            certainty_name(self.inner.certainty),
            self.inner.score,
            self.inner.source.byte.start,
            self.inner.source.byte.end
        )
    }
}

/// 형식은 맞았는데 판정에서 떨어진 후보. "왜 안 걸렸는가"를 설명한다.
#[pyclass(name = "Rejection", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyRejection {
    inner: Rejection,
}

#[pymethods]
impl PyRejection {
    #[getter]
    fn entity(&self) -> &'static str {
        entity_name(self.inner.entity)
    }

    #[getter]
    fn entity_label(&self) -> &'static str {
        self.inner.entity.label()
    }

    #[getter]
    fn source(&self) -> PySpan {
        PySpan { inner: self.inner.source.clone() }
    }

    /// `"checksum_failed"`, `"no_context"`, `"below_threshold"`, `"outranked"` 중 하나.
    #[getter]
    fn reason(&self) -> &'static str {
        reason_name(self.inner.reason)
    }

    #[getter]
    fn score(&self) -> f32 {
        self.inner.score
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Rejection(entity={:?}, reason={:?}, score={:.2})",
            entity_name(self.inner.entity),
            reason_name(self.inner.reason),
            self.inner.score
        )
    }
}

/// 탐지 결과 전체.
#[pyclass(name = "Report", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyReport {
    inner: Report,
}

#[pymethods]
impl PyReport {
    /// 통과한 결과. 원문 위치 오름차순이다.
    #[getter]
    fn findings(&self) -> Vec<PyFinding> {
        self.inner.findings.iter().map(|f| PyFinding { inner: f.clone() }).collect()
    }

    /// 떨어진 후보.
    #[getter]
    fn rejections(&self) -> Vec<PyRejection> {
        self.inner.rejections.iter().map(|r| PyRejection { inner: r.clone() }).collect()
    }

    /// 정규화된 텍스트. 판정 근거를 눈으로 확인할 때 쓴다.
    #[getter]
    fn normalized_text(&self) -> &str {
        &self.inner.normalized_text
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }

    fn __len__(&self) -> usize {
        self.inner.findings.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Report(findings={}, rejections={})",
            self.inner.findings.len(),
            self.inner.rejections.len()
        )
    }
}

// ── 마스킹 정책 ─────────────────────────────────────────────

/// 마스킹 정책. 정적 생성자로 만든다.
///
/// ```python
/// rpit.Policy.redact_label()          # [주민등록번호]
/// rpit.Policy.redact_fill("*")        # **************
/// rpit.Policy.redact_fixed("<가림>")
/// rpit.Policy.partial(3, 4)           # 010******5678
/// rpit.Policy.hash(b"secret", 12)     # hash 기능을 켠 빌드에서만
/// rpit.Policy.tokenize()              # 완전 가역
/// ```
#[pyclass(name = "Policy", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPolicy {
    inner: Policy,
}

#[pymethods]
impl PyPolicy {
    /// **한국어** 이름을 대괄호로 감싼 자리표시자로 통째 치환한다. `[주민등록번호]`
    #[staticmethod]
    fn redact_label() -> Self {
        Self { inner: Policy::Redact(Redaction::Label) }
    }

    /// **영문 대문자** 이름으로 통째 치환한다. `[CREDIT_CARD]`
    ///
    /// 산출물에 한국어가 섞이면 안 되는 경우에 쓴다. 이 라이브러리는 한국어 특화지만
    /// 쓰는 사람까지 한국어 사용자인 것은 아니다.
    #[staticmethod]
    fn redact_code() -> Self {
        Self { inner: Policy::Redact(Redaction::Code) }
    }

    /// 원문 **문자 수**만큼 같은 문자를 반복한다.
    #[staticmethod]
    fn redact_fill(fill: char) -> Self {
        Self { inner: Policy::Redact(Redaction::Fill(fill)) }
    }

    /// 고정 문자열로 바꾼다. 길이도 종류도 남기지 않는다.
    #[staticmethod]
    fn redact_fixed(text: String) -> Self {
        Self { inner: Policy::Redact(Redaction::Fixed(text)) }
    }

    /// 앞뒤 일부만 남기고 가운데를 덮는다. 자릿수는 문자 기준이다.
    #[staticmethod]
    #[pyo3(signature = (keep_prefix, keep_suffix, fill='*'))]
    fn partial(keep_prefix: usize, keep_suffix: usize, fill: char) -> Self {
        Self { inner: Policy::Partial { keep_prefix, keep_suffix, fill } }
    }

    /// 결정적 가명화. 같은 값은 항상 같은 토큰이 되어 연결성 분석이 가능하다.
    ///
    /// `hash` 기능 플래그를 켠 빌드에서만 존재한다.
    #[cfg(feature = "hash")]
    #[staticmethod]
    #[pyo3(signature = (key, length=12))]
    fn hash(key: Vec<u8>, length: usize) -> Self {
        Self { inner: Policy::Hash { key, len: length } }
    }

    /// 토큰화. 완전 가역이며 복원 맵을 함께 낸다.
    #[staticmethod]
    fn tokenize() -> Self {
        Self { inner: Policy::Tokenize }
    }

    fn __repr__(&self) -> String {
        let name = match &self.inner {
            Policy::Redact(Redaction::Label) => "redact_label",
            Policy::Redact(Redaction::Code) => "redact_code",
            Policy::Redact(Redaction::Fill(_)) => "redact_fill",
            Policy::Redact(Redaction::Fixed(_)) => "redact_fixed",
            Policy::Partial { .. } => "partial",
            #[cfg(feature = "hash")]
            Policy::Hash { .. } => "hash",
            Policy::Tokenize => "tokenize",
        };
        format!("Policy.{name}()")
    }
}

/// 엔티티별 정책 묶음. 기본 정책 하나에 엔티티별 예외를 얹는다.
#[pyclass(name = "PolicySet", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPolicySet {
    inner: PolicySet,
}

#[pymethods]
impl PyPolicySet {
    /// 기본 정책을 정한다. 주지 않으면 엔티티 이름 자리표시자 치환이다.
    #[new]
    #[pyo3(signature = (default=None))]
    fn new(default: Option<&PyPolicy>) -> Self {
        match default {
            Some(p) => Self { inner: PolicySet::new(p.inner.clone()) },
            None => Self { inner: PolicySet::default() },
        }
    }

    /// 특정 엔티티에만 다른 정책을 준 **새 묶음**을 낸다. 원본은 바뀌지 않는다.
    fn with_entity(&self, entity: &str, policy: &PyPolicy) -> PyResult<Self> {
        let kind = parse_entity(entity)?;
        Ok(Self { inner: self.inner.clone().with(kind, policy.inner.clone()) })
    }

    /// 이 엔티티에 적용될 정책.
    fn policy_for(&self, entity: &str) -> PyResult<PyPolicy> {
        let kind = parse_entity(entity)?;
        Ok(PyPolicy { inner: self.inner.policy_for(kind).clone() })
    }

    fn __repr__(&self) -> String {
        "PolicySet(...)".to_owned()
    }
}

// ── 복원 ────────────────────────────────────────────────────

/// 토큰 하나와 그것이 대신한 원문 조각.
#[pyclass(name = "RestoreEntry", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyRestoreEntry {
    inner: RestoreEntry,
}

#[pymethods]
impl PyRestoreEntry {
    /// 마스킹 결과에 박힌 토큰.
    #[getter]
    fn token(&self) -> &str {
        &self.inner.token
    }

    /// 그 자리에 있던 원문 조각.
    #[getter]
    fn original(&self) -> &str {
        &self.inner.original
    }

    #[getter]
    fn entity(&self) -> &'static str {
        entity_name(self.inner.entity)
    }

    #[getter]
    fn entity_label(&self) -> &'static str {
        self.inner.entity.label()
    }

    fn __repr__(&self) -> String {
        format!("RestoreEntry(token={:?}, entity={:?})", self.inner.token, entity_name(self.inner.entity))
    }
}

/// 토큰과 원문을 잇는 표.
///
/// **이 표 자체가 개인정보다.** 마스킹 텍스트와 같은 곳에 두면 마스킹한 의미가 없다.
#[pyclass(name = "RestoreMap", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyRestoreMap {
    inner: RestoreMap,
}

#[pymethods]
impl PyRestoreMap {
    /// 이 표가 쓰는 토큰 접두어.
    #[getter]
    fn prefix(&self) -> &str {
        self.inner.prefix()
    }

    /// 등록된 항목들.
    #[getter]
    fn entries(&self) -> Vec<PyRestoreEntry> {
        self.inner.entries().iter().map(|e| PyRestoreEntry { inner: e.clone() }).collect()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 파일에 저장할 수 있게 JSON 으로 낸다.
    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }

    /// 저장했던 JSON 에서 되읽는다. 프로세스가 끝나도 복원이 가능해야 가역이라 부를 수 있다.
    #[staticmethod]
    fn from_json(text: &str) -> PyResult<Self> {
        serde_json::from_str(text)
            .map(|inner| Self { inner })
            .map_err(|e| PyValueError::new_err(format!("restore map JSON 을 읽지 못했다: {e}")))
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __repr__(&self) -> String {
        format!("RestoreMap(prefix={:?}, entries={})", self.inner.prefix(), self.inner.len())
    }
}

/// 마스킹 결과.
#[pyclass(name = "MaskOutput", module = "rust_pii_transformer", skip_from_py_object)]
#[derive(Clone)]
pub struct PyMaskOutput {
    inner: MaskOutput,
}

#[pymethods]
impl PyMaskOutput {
    /// 가려진 텍스트.
    #[getter]
    fn text(&self) -> &str {
        &self.inner.text
    }

    /// 실제로 적용된 탐지 결과.
    #[getter]
    fn applied(&self) -> Vec<PyFinding> {
        self.inner.applied.iter().map(|f| PyFinding { inner: f.clone() }).collect()
    }

    /// 앞선 구간과 겹쳐서 건너뛴 결과. 비어 있는 것이 정상이다.
    #[getter]
    fn skipped(&self) -> Vec<PyFinding> {
        self.inner.skipped.iter().map(|f| PyFinding { inner: f.clone() }).collect()
    }

    /// 토큰화 정책을 쓴 경우의 복원 맵. 그 외에는 `None`.
    #[getter]
    fn restore(&self) -> Option<PyRestoreMap> {
        self.inner.restore.clone().map(|inner| PyRestoreMap { inner })
    }

    fn __repr__(&self) -> String {
        format!(
            "MaskOutput(applied={}, skipped={}, restore={})",
            self.inner.applied.len(),
            self.inner.skipped.len(),
            self.inner.restore.is_some()
        )
    }
}

// ── 모듈 수준 함수 ──────────────────────────────────────────

/// 텍스트를 정규화하고 원문을 잇는 매핑을 함께 낸다.
#[pyfunction]
#[pyo3(name = "normalize", signature = (text, config=None))]
fn py_normalize(text: &str, config: Option<&PyConfig>) -> PyResult<PyNormalized> {
    let cfg = config.map(|c| c.inner.clone()).unwrap_or_default();
    let out = core_normalize(text, &cfg.normalize).map_err(map_err)?;
    Ok(PyNormalized { text: out.text, map: out.map })
}

/// 텍스트에서 개인정보를 탐지한다.
#[pyfunction]
#[pyo3(name = "detect", signature = (text, config=None))]
fn py_detect(text: &str, config: Option<&PyConfig>) -> PyResult<PyReport> {
    let cfg = config.map(|c| c.inner.clone()).unwrap_or_default();
    let report = core_detect(text, &cfg).map_err(map_err)?;
    Ok(PyReport { inner: report })
}

/// 탐지하고 곧바로 마스킹한다.
///
/// `policies` 를 주지 않으면 엔티티 이름 자리표시자로 통째 치환한다.
#[pyfunction]
#[pyo3(name = "mask", signature = (text, policies=None, config=None))]
fn py_mask(
    text: &str,
    policies: Option<&PyPolicySet>,
    config: Option<&PyConfig>,
) -> PyResult<PyMaskOutput> {
    let cfg = config.map(|c| c.inner.clone()).unwrap_or_default();
    let set = policies.map(|p| p.inner.clone()).unwrap_or_default();
    let out = core_mask(text, &cfg, &set).map_err(map_err)?;
    Ok(PyMaskOutput { inner: out })
}

/// 토큰화로 가린 텍스트를 원문으로 되돌린다.
///
/// 토큰 모양이 깨졌거나 표에 없는 번호를 가리키면 `SpanMapError` 를 낸다. 조용히 넘기지 않는
/// 이유는, 복원 실패를 눈치채지 못한 채 원문을 되찾았다고 믿는 것이 이 라이브러리가 막으려는
/// 바로 그 사고이기 때문이다.
#[pyfunction]
#[pyo3(name = "unmask")]
fn py_unmask(masked: &str, restore: &PyRestoreMap) -> PyResult<String> {
    core_unmask(masked, &restore.inner).map_err(map_err)
}

/// 이 빌드가 다루는 엔티티 이름 전부.
#[pyfunction]
fn entity_names() -> Vec<&'static str> {
    crate::detect::entity::ALL.iter().map(|k| k.code()).collect()
}

// ── 모듈 ────────────────────────────────────────────────────

/// Python 모듈 정의. 모듈명은 cdylib 이름(`rust_pii_transformer`)과 일치해야 한다.
#[pymodule]
fn rust_pii_transformer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // 현재 범위를 소비자가 런타임에 확인할 수 있게 남긴다. 없는 기능을 껍데기로 만들지 않는다.
    m.add("__status__", "span, normalize, detect, and mask layers are available")?;
    // 해시 가명화 정책이 이 빌드에 들어 있는지 런타임에 알 수 있게 한다.
    m.add("__has_hash_policy__", cfg!(feature = "hash"))?;

    m.add("SpanMapError", m.py().get_type::<SpanMapError>())?;

    m.add_class::<PySpan>()?;
    m.add_class::<PyNormalizationCost>()?;
    m.add_class::<PySegment>()?;
    m.add_class::<PySourceSpan>()?;
    m.add_class::<PySpanMap>()?;
    m.add_class::<PySpanMapBuilder>()?;

    m.add_class::<PyConfig>()?;
    m.add_class::<PyNormalized>()?;
    m.add_class::<PyContextHit>()?;
    m.add_class::<PyEvidence>()?;
    m.add_class::<PyFinding>()?;
    m.add_class::<PyRejection>()?;
    m.add_class::<PyReport>()?;

    m.add_class::<PyPolicy>()?;
    m.add_class::<PyPolicySet>()?;
    m.add_class::<PyRestoreEntry>()?;
    m.add_class::<PyRestoreMap>()?;
    m.add_class::<PyMaskOutput>()?;

    m.add_function(wrap_pyfunction!(py_normalize, m)?)?;
    m.add_function(wrap_pyfunction!(py_detect, m)?)?;
    m.add_function(wrap_pyfunction!(py_mask, m)?)?;
    m.add_function(wrap_pyfunction!(py_unmask, m)?)?;
    m.add_function(wrap_pyfunction!(entity_names, m)?)?;
    Ok(())
}
