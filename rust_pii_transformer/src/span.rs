//! 정규화 전후를 잇는 단조(monotone) 구간 정렬 테이블.
//!
//! 이 모듈이 이 라이브러리의 중심이다. 정규화는 문자를 지우고(하이픈 흡수), 바꾸고(전각을 반각으로),
//! 늘리고(`일억` 이 숫자 9자리로), 줄인다(`구십일` 이 숫자 2자리로). 그래서 원문과 정규화문 사이에
//! 1대1 대응이 성립하지 않는다. 그러나 **순서는 항상 보존된다.** 이 단조성이 해법의 기반이다.
//!
//! ## 왜 문자별 인덱스 배열이 아니라 세그먼트 테이블인가
//!
//! 문자별 배열은 O(1) 조회를 주지만 문자당 8바이트가 들고, 무엇보다 "이 구간은 통째로 하나의
//! 변환 결과"라는 원자성과 "어떤 규칙이 이 구간을 만들었는가"라는 출처를 담지 못한다.
//! 판정 근거 출력이 불가능해진다. 세그먼트 테이블은 항등 구간을 하나로 접으므로 실텍스트에서
//! 항목 수가 수십 개 수준이고, 이진 탐색으로 조회하며, 규칙 출처와 비용을 함께 싣는다.
//!
//! ## 경계 스냅
//!
//! 탐지 스팬이 비항등 세그먼트를 가로지르면 세그먼트 경계 바깥으로 넓히고
//! [`SourceSpan::snapped`] 에 그 사실을 남긴다. 예를 들어 `일억` 이 `100000000` 으로 늘어난 뒤
//! 그 안의 `100000` 만 매칭되면, 원문 스팬은 `일억` 전체가 되고 `snapped` 가 참이 된다.
//!
//! ## 불변식
//!
//! [`SpanMap::validate`] 가 강제한다. 세그먼트는 양쪽 좌표계에서 정렬되어 있고 빈틈과 겹침이
//! 없으며 텍스트 전체를 덮는다. 항등 구간은 양쪽 길이가 같고 규칙과 비용이 없다.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 정규화 규칙 식별자. 판정 근거에 그대로 실린다.
///
/// 각 정규화 패스가 자기 규칙 이름을 정적 문자열로 넘긴다(예: `"separator.hyphen"`).
///
/// 정적 문자열이라 규칙 이름에 힙 할당이 전혀 없다. 그 대가로 규칙을 담는 타입
/// ([`Segment`] · [`SourceSpan`] · [`SpanMap`])은 `Serialize` 만 유도하고 `Deserialize` 는
/// 유도하지 않는다. `Vec<&'static str>` 을 역직렬화하려면 입력 수명이 `'static` 이어야 하는데
/// 그것을 만족시킬 방법이 없기 때문이다. 이 크레이트의 용도는 판정 결과를 JSON 으로 **내보내는**
/// 것이고 되읽는 것이 아니므로 이 교환이 맞다.
pub type RuleId = &'static str;

// ── 기본 타입 ────────────────────────────────────────────────

/// 텍스트 한 구간. 바이트와 문자 오프셋을 **둘 다** 담는다.
///
/// Rust 문자열 슬라이싱은 바이트 인덱스를 쓰고 Python 과 JavaScript 소비자는 문자 인덱스를 쓴다.
/// 한국어는 음절 1자가 UTF-8 로 3바이트라 둘이 항상 다르다. 나중에 변환하면 매번 O(n) 스캔이
/// 붙고 그 변환이 새로운 오프셋 버그의 출처가 되므로 처음부터 함께 들고 다닌다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// UTF-8 바이트 오프셋 구간.
    pub byte: Range<u32>,
    /// 유니코드 스칼라 값(문자) 오프셋 구간.
    pub char: Range<u32>,
}

impl Span {
    /// 바이트 구간과 문자 구간으로 스팬을 만든다.
    pub fn new(byte: Range<u32>, char: Range<u32>) -> Self {
        Self { byte, char }
    }

    /// 주어진 위치의 폭 0 스팬.
    pub fn empty_at(byte: u32, char: u32) -> Self {
        Self { byte: byte..byte, char: char..char }
    }

    /// 폭이 0인가.
    pub fn is_empty(&self) -> bool {
        self.byte.start >= self.byte.end
    }

    /// 바이트 길이.
    pub fn byte_len(&self) -> u32 {
        self.byte.end.saturating_sub(self.byte.start)
    }

    /// 문자 길이.
    pub fn char_len(&self) -> u32 {
        self.char.end.saturating_sub(self.char.start)
    }

    /// 이 스팬이 가리키는 원문 조각을 잘라 낸다.
    ///
    /// # Panics
    ///
    /// 오프셋이 `text` 범위를 벗어나거나 문자 경계가 아니면 패닉한다.
    /// 이 크레이트가 만든 스팬을 그 스팬이 나온 텍스트에 쓰는 한 발생하지 않는다.
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        &text[self.byte.start as usize..self.byte.end as usize]
    }
}

/// 세그먼트 한 조각이 무슨 변환인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// 원문 그대로 통과.
    Identity,
    /// 치환. 전각을 반각으로, 유사문자 교정 등. 문자 수가 같거나 줄어든다.
    Replace,
    /// 원문에만 존재. 하이픈, 공백, 점 흡수.
    Delete,
    /// 정규화문이 더 길다. 한글 수사를 숫자로 펼친 경우.
    Expand,
}

// 삽입(원문에 대응이 없는 문자를 끼워 넣는 것)은 이 자료구조가 지원하지 않는다.
//
// 처음에는 확장 대비로 두었으나 실측으로 원리적 충돌이 드러나 걷어냈다. 정규화 비용은
// **원문을 기준으로** 정의되는데, 끼워 넣은 문자에는 대응하는 원문이 없다. 앞 패스가 넣은
// 문자를 뒤 패스가 흡수하면 양쪽이 모두 빈 조각이 남고, 그 조각의 비용을 어느 원문 구간에
// 매길지 결정할 방법이 없다. 합성을 어느 순서로 묶느냐에 따라 비용이 달라져 결정성이 깨졌다.
//
// 정규화 패스가 문자를 끼워 넣을 이유도 없다. 네 패스는 모두 통과·치환·펼침·흡수뿐이다.
// 쓰지도 않는 경로를 위해 비용 모델을 흐리지 않는다.

/// 삭제된 원문 조각의 종류. 신뢰도 감점 계수가 종류마다 다르다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Absorbed {
    /// 공백. 없던 숫자열을 만들 수 있어 가장 위험하다.
    Whitespace,
    /// 하이픈, 점, 괄호 같은 구분자.
    Separator,
    /// 그 밖의 제어 문자 등.
    Other,
}

/// 이 스팬이 원문에서 얼마나 멀어졌는지의 정량 지표.
///
/// 탐지 층이 신뢰도를 깎는 데 쓴다. "정규화는 관대하게, 검증은 엄격하게" 원칙의 대가를
/// 계량화한 것이다. 계수는 탐지 층이 정하고 이 구조체는 횟수만 센다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NormalizationCost {
    /// 흡수한 공백 문자 수.
    pub absorbed_whitespace: u16,
    /// 흡수한 구분자 수.
    pub absorbed_separators: u16,
    /// 숫자로 펼쳐진 한글 음절 수.
    pub expanded_syllables: u16,
    /// 치환된 문자 수.
    pub replaced_chars: u16,
}

impl NormalizationCost {
    /// 아무 변환도 없었는가.
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }

    /// 다른 비용을 더한다. u16 상한에서 포화한다.
    pub fn add(&mut self, other: &Self) {
        self.absorbed_whitespace = self.absorbed_whitespace.saturating_add(other.absorbed_whitespace);
        self.absorbed_separators = self.absorbed_separators.saturating_add(other.absorbed_separators);
        self.expanded_syllables = self.expanded_syllables.saturating_add(other.expanded_syllables);
        self.replaced_chars = self.replaced_chars.saturating_add(other.replaced_chars);
    }
}

/// 정규화 한 조각. 연속한 [`SegmentKind::Identity`] 구간은 하나로 접어 저장한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    /// 입력(원문) 쪽 구간.
    pub src: Span,
    /// 출력(정규화문) 쪽 구간.
    pub dst: Span,
    pub kind: SegmentKind,
    /// 이 구간을 만든 규칙들. 항등 구간은 비어 있다.
    ///
    /// **적용 순서가 아니라 이름 오름차순**으로 정규화된다. 합성을 어떤 순서로 묶든
    /// 같은 결과가 나오게 하기 위해서다. 자세한 이유는 [`SourceSpan::rules`] 참조.
    pub rules: Vec<RuleId>,
    /// 이 구간 하나의 정규화 비용.
    pub cost: NormalizationCost,
}

/// [`SpanMap::to_source`] 의 결과. 원문 스팬과 그 판정 근거.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSpan {
    /// 원문 기준 스팬.
    pub span: Span,
    /// 세그먼트 경계 바깥으로 넓혀졌는가.
    pub snapped: bool,
    /// 이 구간에 적용된 정규화 규칙 목록. 중복은 제거되고 **이름 오름차순**으로 정렬된다.
    ///
    /// 적용 순서로 두지 않는 이유가 있다. `compose(compose(a, b), c)` 와
    /// `compose(a, compose(b, c))` 는 최종 스팬·종류·비용이 모두 같지만, 중간 단계에서
    /// 세그먼트가 묶이는 굵기가 달라 규칙이 수집되는 순서가 달라진다. 순서를 그대로 두면
    /// 파이프라인을 어떻게 조립했는지에 따라 출력이 달라져 결정성이 깨진다.
    /// 판정 근거로서 필요한 것은 "어떤 규칙들이 관여했는가"라는 집합이지 순서가 아니므로,
    /// 정렬로 정규화해 합성 결합법칙을 완전하게 만든다.
    pub rules: Vec<RuleId>,
    /// 이 구간에 누적된 정규화 비용.
    pub cost: NormalizationCost,
}

// ── SpanMap ─────────────────────────────────────────────────

/// 원문과 정규화문을 잇는 단조 구간 정렬 테이블.
///
/// 세그먼트는 양쪽 좌표계에서 정렬되어 있고 빈틈과 겹침 없이 텍스트 전체를 덮는다.
/// 만드는 방법은 [`SpanMapBuilder`] 또는 [`SpanMap::identity`] 다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SpanMap {
    segments: Vec<Segment>,
}

impl SpanMap {
    /// 변환이 전혀 없는 항등 매핑.
    pub fn identity(text: &str) -> Self {
        let mut builder = SpanMapBuilder::with_capacity(text);
        builder.keep(text);
        builder.finish().1
    }

    /// 세그먼트 목록.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// 변환이 하나도 없는가.
    pub fn is_identity(&self) -> bool {
        self.segments.iter().all(|s| s.kind == SegmentKind::Identity)
    }

    /// 원문 끝 좌표 (바이트, 문자).
    pub fn src_end(&self) -> (u32, u32) {
        self.segments
            .last()
            .map(|s| (s.src.byte.end, s.src.char.end))
            .unwrap_or((0, 0))
    }

    /// 정규화문 끝 좌표 (바이트, 문자).
    pub fn dst_end(&self) -> (u32, u32) {
        self.segments
            .last()
            .map(|s| (s.dst.byte.end, s.dst.char.end))
            .unwrap_or((0, 0))
    }

    /// 정규화문 구간을 원문 구간으로 되돌린다.
    ///
    /// 항등 구간 안에서는 부분 매핑이 되고, 비항등 구간을 가로지르면 경계 바깥으로 넓힌 뒤
    /// [`SourceSpan::snapped`] 를 참으로 만든다.
    ///
    /// 스팬 경계에 딱 붙은 폭 0 세그먼트(예: 매치 바로 앞의 흡수된 하이픈)는 포함하지 않는다.
    /// `-880101` 에서 `880101` 을 찾으면 원문 스팬은 하이픈을 뺀 `880101` 이다.
    /// 반대로 스팬 안쪽의 흡수된 하이픈은 포함된다.
    pub fn to_source(&self, dst: &Span) -> SourceSpan {
        if self.segments.is_empty() {
            return SourceSpan {
                span: Span::empty_at(0, 0),
                snapped: false,
                rules: Vec::new(),
                cost: NormalizationCost::default(),
            };
        }

        let start = dst.byte.start;
        let end = dst.byte.end;

        // 시작 위치 이전에서 끝나는 세그먼트는 건너뛴다. 경계의 폭 0 세그먼트가 여기서 걸러진다.
        let first_idx = self.segments.partition_point(|s| s.dst.byte.end <= start);
        // 끝 위치 이후에서 시작하는 세그먼트는 제외한다.
        let after_idx = self.segments.partition_point(|s| s.dst.byte.start < end);

        if first_idx >= after_idx {
            // 폭 0 질의이거나 어떤 세그먼트도 덮지 않는다. 위치만 되돌린다.
            let idx = first_idx.min(self.segments.len() - 1);
            let (b, c) = self
                .backward_point(idx, start, dst.char.start)
                .unwrap_or_else(|_| self.src_end());
            return SourceSpan {
                span: Span::empty_at(b, c),
                snapped: false,
                rules: Vec::new(),
                cost: NormalizationCost::default(),
            };
        }

        let first = &self.segments[first_idx];
        let last = &self.segments[after_idx - 1];
        let mut snapped = false;

        let (src_byte_start, src_char_start) =
            if first.kind == SegmentKind::Identity && start > first.dst.byte.start {
                (
                    first.src.byte.start + (start - first.dst.byte.start),
                    first.src.char.start + (dst.char.start - first.dst.char.start),
                )
            } else {
                snapped |= start != first.dst.byte.start;
                (first.src.byte.start, first.src.char.start)
            };

        let (src_byte_end, src_char_end) =
            if last.kind == SegmentKind::Identity && end < last.dst.byte.end {
                (
                    last.src.byte.start + (end - last.dst.byte.start),
                    last.src.char.start + (dst.char.end - last.dst.char.start),
                )
            } else {
                snapped |= end != last.dst.byte.end;
                (last.src.byte.end, last.src.char.end)
            };

        let mut rules: Vec<RuleId> = Vec::new();
        let mut cost = NormalizationCost::default();
        for seg in &self.segments[first_idx..after_idx] {
            cost.add(&seg.cost);
            rules.extend_from_slice(&seg.rules);
        }
        canonicalize_rules(&mut rules);

        SourceSpan {
            span: Span::new(src_byte_start..src_byte_end, src_char_start..src_char_end),
            snapped,
            rules,
            cost,
        }
    }

    /// 두 매핑을 하나로 합친다.
    ///
    /// `inner` 가 A 를 B 로, `outer` 가 B 를 C 로 옮긴다면 결과는 A 를 C 로 옮긴다.
    /// 두 테이블 모두 단조이므로 단일 병합 순회로 끝난다.
    ///
    /// 한쪽의 세그먼트 경계가 다른 쪽 비항등 세그먼트의 내부에 떨어지면 자르지 않고 **묶는다.**
    /// 원자성을 지키기 위해서다. 그 대신 결과 세그먼트가 굵어진다.
    ///
    /// # Errors
    ///
    /// `inner` 의 출력 좌표계와 `outer` 의 입력 좌표계가 다르면 [`Error::SpanMapMismatch`].
    pub fn compose(inner: &SpanMap, outer: &SpanMap) -> Result<SpanMap> {
        if inner.dst_end() != outer.src_end() {
            return Err(Error::mismatch(format!(
                "inner output {:?} != outer input {:?}",
                inner.dst_end(),
                outer.src_end()
            )));
        }

        let (ni, no) = (inner.segments.len(), outer.segments.len());
        let mut out: Vec<Segment> = Vec::with_capacity(ni.max(no));
        let (mut i, mut j) = (0usize, 0usize);
        // B 좌표의 현재 위치. 항상 양쪽에서 합법인 컷 지점이다.
        let (mut bb, mut bc) = (0u32, 0u32);

        while i < ni || j < no {
            // B 폭이 0인 inner 세그먼트(원문에만 있는 것)는 그대로 통과시킨다.
            // 묶어 버리면 to_source 의 경계 규칙이 무너지므로 독립 세그먼트로 남긴다.
            if i < ni && inner.segments[i].dst.is_empty() {
                let seg = &inner.segments[i];
                let (cb, cc) = outer.forward_point(j, bb, bc)?;
                out.push(Segment {
                    src: seg.src.clone(),
                    dst: Span::empty_at(cb, cc),
                    kind: SegmentKind::Delete,
                    rules: seg.rules.clone(),
                    cost: seg.cost,
                });
                i += 1;
                continue;
            }

            // 원문 쪽이 빈 세그먼트는 존재할 수 없다(삽입 미지원). 만들어졌다면 손상된 표다.
            if j < no && outer.segments[j].src.is_empty() {
                return Err(Error::mismatch(format!(
                    "outer segment {j} has an empty source range; insertion is not supported"
                )));
            }

            if i >= ni || j >= no {
                return Err(Error::mismatch(format!(
                    "segment exhausted at B byte {bb} (inner {i}/{ni}, outer {j}/{no})"
                )));
            }

            // 합법 컷이 나올 때까지 그룹을 넓힌다.
            let (gi, gj) = (i, j);
            let mut ei = inner.segments[i].dst.byte.end;
            let mut ei_c = inner.segments[i].dst.char.end;
            let mut ej = outer.segments[j].src.byte.end;
            let mut ej_c = outer.segments[j].src.char.end;

            let (qb, qc) = loop {
                if ei == ej {
                    break (ei, ei_c);
                }
                if ei < ej {
                    // ei 에서 자르려면 outer 쪽이 항등이어야 한다.
                    if outer.segments[j].kind == SegmentKind::Identity {
                        break (ei, ei_c);
                    }
                    i += 1;
                    if i >= ni {
                        return Err(Error::mismatch(format!("inner exhausted while grouping at B byte {bb}")));
                    }
                    ei = inner.segments[i].dst.byte.end;
                    ei_c = inner.segments[i].dst.char.end;
                } else {
                    if inner.segments[i].kind == SegmentKind::Identity {
                        break (ej, ej_c);
                    }
                    j += 1;
                    if j >= no {
                        return Err(Error::mismatch(format!("outer exhausted while grouping at B byte {bb}")));
                    }
                    ej = outer.segments[j].src.byte.end;
                    ej_c = outer.segments[j].src.char.end;
                }
            };

            let (ab0, ac0) = inner.backward_point(gi, bb, bc)?;
            let (ab1, ac1) = inner.backward_point(i, qb, qc)?;
            let (cb0, cc0) = outer.forward_point(gj, bb, bc)?;
            let (cb1, cc1) = outer.forward_point(j, qb, qc)?;

            let mut rules: Vec<RuleId> = Vec::new();
            let mut cost = NormalizationCost::default();
            let mut all_identity = true;
            for seg in &inner.segments[gi..=i] {
                if seg.kind != SegmentKind::Identity {
                    all_identity = false;
                    cost.add(&seg.cost);
                    rules.extend_from_slice(&seg.rules);
                }
            }
            for seg in &outer.segments[gj..=j] {
                if seg.kind != SegmentKind::Identity {
                    all_identity = false;
                    cost.add(&seg.cost);
                    rules.extend_from_slice(&seg.rules);
                }
            }
            canonicalize_rules(&mut rules);

            let src = Span::new(ab0..ab1, ac0..ac1);
            let dst = Span::new(cb0..cb1, cc0..cc1);

            let kind = if all_identity {
                SegmentKind::Identity
            } else {
                classify(&src, &dst)
            };

            out.push(Segment { src, dst, kind, rules, cost });

            // 완전히 소비된 세그먼트만 넘긴다. 부분 소비는 항등 구간에서만 일어난다.
            if inner.segments[i].dst.byte.end <= qb {
                i += 1;
            }
            if outer.segments[j].src.byte.end <= qb {
                j += 1;
            }
            bb = qb;
            bc = qc;
        }

        Ok(SpanMap { segments: merge_adjacent_identity(out) })
    }

    /// 불변식 검사. 정규화 패스를 새로 만들 때 테스트에서 이것을 통과시켜야 한다.
    ///
    /// # Errors
    ///
    /// 정렬·연속·피복이 깨졌거나 세그먼트 종류와 실제 길이가 어긋나면 [`Error::SpanMapInvariant`].
    pub fn validate(&self) -> Result<()> {
        let (mut sb, mut sc, mut db, mut dc) = (0u32, 0u32, 0u32, 0u32);

        for (index, seg) in self.segments.iter().enumerate() {
            if seg.src.byte.start != sb || seg.src.char.start != sc {
                return Err(Error::invariant(
                    index,
                    format!("src not contiguous: expected byte {sb} char {sc}, found byte {} char {}", seg.src.byte.start, seg.src.char.start),
                ));
            }
            if seg.dst.byte.start != db || seg.dst.char.start != dc {
                return Err(Error::invariant(
                    index,
                    format!("dst not contiguous: expected byte {db} char {dc}, found byte {} char {}", seg.dst.byte.start, seg.dst.char.start),
                ));
            }
            if seg.src.byte.end < seg.src.byte.start || seg.dst.byte.end < seg.dst.byte.start {
                return Err(Error::invariant(index, "reversed byte range"));
            }
            if seg.src.char.end < seg.src.char.start || seg.dst.char.end < seg.dst.char.start {
                return Err(Error::invariant(index, "reversed char range"));
            }

            match seg.kind {
                SegmentKind::Identity => {
                    if seg.src.byte_len() != seg.dst.byte_len() || seg.src.char_len() != seg.dst.char_len() {
                        return Err(Error::invariant(index, "identity segment has unequal lengths"));
                    }
                    if !seg.rules.is_empty() || !seg.cost.is_zero() {
                        return Err(Error::invariant(index, "identity segment carries rules or cost"));
                    }
                }
                SegmentKind::Delete => {
                    if !seg.dst.is_empty() || seg.src.is_empty() {
                        return Err(Error::invariant(index, "delete segment must consume src and produce nothing"));
                    }
                }
                SegmentKind::Expand => {
                    if seg.dst.char_len() <= seg.src.char_len() {
                        return Err(Error::invariant(index, "expand segment must grow in chars"));
                    }
                }
                SegmentKind::Replace => {
                    if seg.src.is_empty() || seg.dst.is_empty() {
                        return Err(Error::invariant(index, "replace segment must have both sides"));
                    }
                }
            }

            // 삽입은 지원하지 않으므로 원문 쪽이 빈 세그먼트는 어떤 종류로도 존재할 수 없다.
            if seg.src.is_empty() {
                return Err(Error::invariant(
                    index,
                    "segment has an empty source range; insertion is not supported",
                ));
            }

            sb = seg.src.byte.end;
            sc = seg.src.char.end;
            db = seg.dst.byte.end;
            dc = seg.dst.char.end;
        }

        Ok(())
    }

    /// 정규화문 위치 하나를 원문 위치로 되돌린다. `idx` 는 그 위치를 담당하는 세그먼트 인덱스다.
    fn backward_point(&self, idx: usize, p_byte: u32, p_char: u32) -> Result<(u32, u32)> {
        if idx >= self.segments.len() {
            return Ok(self.src_end());
        }
        let seg = &self.segments[idx];
        if seg.dst.byte.start == p_byte {
            return Ok((seg.src.byte.start, seg.src.char.start));
        }
        if seg.dst.byte.end == p_byte {
            return Ok((seg.src.byte.end, seg.src.char.end));
        }
        if seg.kind == SegmentKind::Identity && seg.dst.byte.start < p_byte && p_byte < seg.dst.byte.end {
            return Ok((
                seg.src.byte.start + (p_byte - seg.dst.byte.start),
                seg.src.char.start + (p_char - seg.dst.char.start),
            ));
        }
        Err(Error::mismatch(format!(
            "dst byte {p_byte} falls inside non-identity segment {idx}"
        )))
    }

    /// 원문 위치 하나를 정규화문 위치로 옮긴다. `idx` 는 그 위치를 담당하는 세그먼트 인덱스다.
    fn forward_point(&self, idx: usize, p_byte: u32, p_char: u32) -> Result<(u32, u32)> {
        if idx >= self.segments.len() {
            return Ok(self.dst_end());
        }
        let seg = &self.segments[idx];
        if seg.src.byte.start == p_byte {
            return Ok((seg.dst.byte.start, seg.dst.char.start));
        }
        if seg.src.byte.end == p_byte {
            return Ok((seg.dst.byte.end, seg.dst.char.end));
        }
        if seg.kind == SegmentKind::Identity && seg.src.byte.start < p_byte && p_byte < seg.src.byte.end {
            return Ok((
                seg.dst.byte.start + (p_byte - seg.src.byte.start),
                seg.dst.char.start + (p_char - seg.src.char.start),
            ));
        }
        Err(Error::mismatch(format!(
            "src byte {p_byte} falls inside non-identity segment {idx}"
        )))
    }
}

// ── SpanMapBuilder ──────────────────────────────────────────

/// 정규화 패스가 출력 문자열과 [`SpanMap`] 을 동시에 만드는 빌더.
///
/// 패스는 원문을 앞에서부터 훑으면서 조각마다 [`keep`](Self::keep) ·
/// [`replace`](Self::replace) · [`numeral`](Self::numeral) · [`absorb`](Self::absorb) ·
/// [`insert`](Self::insert) 중 하나를 호출한다. 원문 조각을 빠짐없이 한 번씩 넘겨야
/// 피복 불변식이 성립한다.
#[derive(Debug, Default)]
pub struct SpanMapBuilder {
    segments: Vec<Segment>,
    out: String,
    src_byte: u32,
    src_char: u32,
    dst_byte: u32,
    dst_char: u32,
}

impl SpanMapBuilder {
    /// 빈 빌더.
    pub fn new() -> Self {
        Self::default()
    }

    /// 출력 버퍼를 입력 길이만큼 미리 잡아 둔 빌더.
    pub fn with_capacity(text: &str) -> Self {
        Self { out: String::with_capacity(text.len()), ..Default::default() }
    }

    /// 원문 조각을 그대로 통과시킨다. 연속 호출은 하나의 항등 세그먼트로 접힌다.
    pub fn keep(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let src = self.advance_src(text);
        let dst = self.advance_dst(text);

        if let Some(last) = self.segments.last_mut() {
            if last.kind == SegmentKind::Identity {
                last.src.byte.end = src.byte.end;
                last.src.char.end = src.char.end;
                last.dst.byte.end = dst.byte.end;
                last.dst.char.end = dst.char.end;
                return;
            }
        }
        self.segments.push(Segment {
            src,
            dst,
            kind: SegmentKind::Identity,
            rules: Vec::new(),
            cost: NormalizationCost::default(),
        });
    }

    /// 문자 폴딩. 전각을 반각으로 바꾸거나 유사문자를 교정할 때 쓴다.
    pub fn replace(&mut self, src_text: &str, dst_text: &str, rule: RuleId) {
        // 원문 조각이 비면 삽입이 되는데 그것은 지원하지 않는다. 조용히 넘긴다.
        if src_text.is_empty() {
            return;
        }
        let src = self.advance_src(src_text);
        let dst = self.advance_dst(dst_text);
        let cost = NormalizationCost {
            replaced_chars: clamp_u16(src.char_len()),
            ..Default::default()
        };
        self.push_transform(src, dst, rule, cost);
    }

    /// 한글 수사 역변환. `팔팔공일공일` 을 `880101` 로, `구십일` 을 `91` 로 옮길 때 쓴다.
    ///
    /// 결과가 짧아지는 경우(`구십일` → `91`)도 있으므로 종류는 길이로 판정하고,
    /// 비용은 소비한 한글 음절 수로 센다.
    pub fn numeral(&mut self, src_text: &str, dst_text: &str, rule: RuleId) {
        // 원문 조각이 비면 삽입이 되는데 그것은 지원하지 않는다. 조용히 넘긴다.
        if src_text.is_empty() {
            return;
        }
        let src = self.advance_src(src_text);
        let dst = self.advance_dst(dst_text);
        let cost = NormalizationCost {
            expanded_syllables: clamp_u16(src.char_len()),
            ..Default::default()
        };
        self.push_transform(src, dst, rule, cost);
    }

    /// 원문 조각을 흡수(삭제)한다. 하이픈, 공백, 점 같은 구분자를 지울 때 쓴다.
    pub fn absorb(&mut self, src_text: &str, rule: RuleId, class: Absorbed) {
        if src_text.is_empty() {
            return;
        }
        let src = self.advance_src(src_text);
        let dst = Span::empty_at(self.dst_byte, self.dst_char);
        let n = clamp_u16(src.char_len());
        let cost = match class {
            Absorbed::Whitespace => NormalizationCost { absorbed_whitespace: n, ..Default::default() },
            Absorbed::Separator => NormalizationCost { absorbed_separators: n, ..Default::default() },
            Absorbed::Other => NormalizationCost::default(),
        };
        self.push_transform(src, dst, rule, cost);
    }

    /// 정규화 결과 문자열과 매핑을 낸다.
    pub fn finish(self) -> (String, SpanMap) {
        (self.out, SpanMap { segments: self.segments })
    }

    fn push_transform(&mut self, src: Span, dst: Span, rule: RuleId, cost: NormalizationCost) {
        let kind = classify(&src, &dst);
        self.segments.push(Segment { src, dst, kind, rules: vec![rule], cost });
    }

    fn advance_src(&mut self, text: &str) -> Span {
        let (b0, c0) = (self.src_byte, self.src_char);
        self.src_byte += text.len() as u32;
        self.src_char += text.chars().count() as u32;
        Span::new(b0..self.src_byte, c0..self.src_char)
    }

    fn advance_dst(&mut self, text: &str) -> Span {
        let (b0, c0) = (self.dst_byte, self.dst_char);
        self.out.push_str(text);
        self.dst_byte += text.len() as u32;
        self.dst_char += text.chars().count() as u32;
        Span::new(b0..self.dst_byte, c0..self.dst_char)
    }
}

// ── 내부 헬퍼 ────────────────────────────────────────────────

/// 양쪽 길이로 세그먼트 종류를 정한다. 원문 쪽은 항상 비어 있지 않다(삽입 미지원).
fn classify(src: &Span, dst: &Span) -> SegmentKind {
    debug_assert!(!src.is_empty(), "삽입은 지원하지 않는다");
    if dst.is_empty() {
        SegmentKind::Delete
    } else if dst.char_len() > src.char_len() {
        SegmentKind::Expand
    } else {
        SegmentKind::Replace
    }
}

fn clamp_u16(n: u32) -> u16 {
    n.min(u16::MAX as u32) as u16
}

/// 규칙 목록을 이름 오름차순으로 정렬하고 중복을 지운다.
///
/// 합성 결합법칙을 완전하게 만드는 장치다. 근거는 [`SourceSpan::rules`] 문서 참조.
fn canonicalize_rules(rules: &mut Vec<RuleId>) {
    rules.sort_unstable();
    rules.dedup();
}

/// compose 가 항등 구간을 여러 조각으로 쪼갠 경우 다시 접는다.
fn merge_adjacent_identity(segments: Vec<Segment>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::with_capacity(segments.len());
    for seg in segments {
        if seg.kind == SegmentKind::Identity {
            if let Some(last) = out.last_mut() {
                if last.kind == SegmentKind::Identity {
                    last.src.byte.end = seg.src.byte.end;
                    last.src.char.end = seg.src.char.end;
                    last.dst.byte.end = seg.dst.byte.end;
                    last.dst.char.end = seg.dst.char.end;
                    continue;
                }
            }
        }
        out.push(seg);
    }
    out
}

// ── 테스트 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const R_SEP: RuleId = "test.separator";
    const R_FOLD: RuleId = "test.fold";
    const R_NUM: RuleId = "test.numeral";

    /// 전체 dst 를 질의하는 스팬.
    fn whole(map: &SpanMap) -> Span {
        let (b, c) = map.dst_end();
        Span::new(0..b, 0..c)
    }

    #[test]
    fn identity_map_is_valid_and_transparent() {
        let text = "주민등록번호 880101";
        let map = SpanMap::identity(text);
        map.validate().unwrap();
        assert!(map.is_identity());
        assert_eq!(map.segments().len(), 1);

        let got = map.to_source(&whole(&map));
        assert_eq!(got.span.byte, 0..text.len() as u32);
        assert_eq!(got.span.char, 0..text.chars().count() as u32);
        assert!(!got.snapped);
        assert!(got.rules.is_empty());
        assert!(got.cost.is_zero());
    }

    #[test]
    fn empty_text_produces_empty_map() {
        let map = SpanMap::identity("");
        map.validate().unwrap();
        assert!(map.segments().is_empty());
        assert_eq!(map.dst_end(), (0, 0));

        let got = map.to_source(&Span::empty_at(0, 0));
        assert!(got.span.is_empty());
    }

    #[test]
    fn absorbed_hyphen_inside_span_is_included() {
        // "880101-1234567" → "8801011234567"
        let mut b = SpanMapBuilder::new();
        b.keep("880101");
        b.absorb("-", R_SEP, Absorbed::Separator);
        b.keep("1234567");
        let (out, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(out, "8801011234567");

        let got = map.to_source(&whole(&map));
        assert_eq!(got.span.byte, 0..14);
        assert!(!got.snapped);
        assert_eq!(got.rules, vec![R_SEP]);
        assert_eq!(got.cost.absorbed_separators, 1);
    }

    #[test]
    fn absorbed_hyphen_at_span_boundary_is_excluded() {
        // "-880101" → "880101". 매치는 dst 0..6 이고 앞의 하이픈은 스팬 밖이다.
        let mut b = SpanMapBuilder::new();
        b.absorb("-", R_SEP, Absorbed::Separator);
        b.keep("880101");
        let (out, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(out, "880101");

        let got = map.to_source(&Span::new(0..6, 0..6));
        assert_eq!(got.span.byte, 1..7, "선행 하이픈은 원문 스팬에 들어가면 안 된다");
        assert!(!got.snapped);
        assert!(got.rules.is_empty());
        assert!(got.cost.is_zero());
    }

    #[test]
    fn trailing_absorbed_separator_is_excluded() {
        let mut b = SpanMapBuilder::new();
        b.keep("880101");
        b.absorb("-", R_SEP, Absorbed::Separator);
        let (out, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(out, "880101");

        let got = map.to_source(&Span::new(0..6, 0..6));
        assert_eq!(got.span.byte, 0..6);
        assert!(!got.snapped);
    }

    #[test]
    fn partial_match_inside_expansion_snaps_outward() {
        // "일억" → "100000000". 그 안의 "100000" 만 매칭되면 원문은 "일억" 전체가 된다.
        let mut b = SpanMapBuilder::new();
        b.numeral("일억", "100000000", R_NUM);
        let (out, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(out, "100000000");
        assert_eq!(map.segments()[0].kind, SegmentKind::Expand);

        let got = map.to_source(&Span::new(0..6, 0..6));
        assert_eq!(got.span.byte, 0..6, "'일억' 은 UTF-8 로 6바이트다");
        assert_eq!(got.span.char, 0..2);
        assert!(got.snapped, "확장 구간을 반으로 가르면 스냅이 일어나야 한다");
        assert_eq!(got.cost.expanded_syllables, 2);
    }

    #[test]
    fn shrinking_numeral_is_replace_not_expand() {
        // "구십일" → "91" 은 줄어든다. 종류는 Replace 지만 비용은 음절 수로 센다.
        let mut b = SpanMapBuilder::new();
        b.numeral("구십일", "91", R_NUM);
        let (_, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(map.segments()[0].kind, SegmentKind::Replace);
        assert_eq!(map.segments()[0].cost.expanded_syllables, 3);
    }

    #[test]
    fn korean_byte_and_char_offsets_stay_separate() {
        // 한글 음절은 3바이트, 숫자는 1바이트다. 두 좌표가 다르게 흘러야 한다.
        let mut b = SpanMapBuilder::new();
        b.keep("생년월일 ");
        b.numeral("팔팔공일공일", "880101", R_NUM);
        let (out, map) = b.finish();
        map.validate().unwrap();
        assert_eq!(out, "생년월일 880101");

        // dst 에서 "880101" 은 바이트 13..19, 문자 5..11.
        let got = map.to_source(&Span::new(13..19, 5..11));
        assert_eq!(got.span.byte, 13..31, "원문의 '팔팔공일공일' 은 18바이트다");
        assert_eq!(got.span.char, 5..11);
        assert!(!got.snapped);
        assert_eq!(got.cost.expanded_syllables, 6);
    }

    #[test]
    fn partial_match_inside_identity_does_not_snap() {
        let map = SpanMap::identity("abcdefghij");
        let got = map.to_source(&Span::new(3..7, 3..7));
        assert_eq!(got.span.byte, 3..7);
        assert!(!got.snapped);
    }

    #[test]
    fn rules_are_deduplicated() {
        let mut b = SpanMapBuilder::new();
        b.keep("1");
        b.absorb("-", R_SEP, Absorbed::Separator);
        b.keep("2");
        b.absorb("-", R_SEP, Absorbed::Separator);
        b.keep("3");
        let (_, map) = b.finish();
        map.validate().unwrap();

        let got = map.to_source(&whole(&map));
        assert_eq!(got.rules, vec![R_SEP], "같은 규칙이 두 번 나와도 한 번만 실린다");
        assert_eq!(got.cost.absorbed_separators, 2, "비용은 누적된다");
    }

    // ── compose ────────────────────────────────────────────

    /// 전각 숫자를 반각으로 접는 1번 패스.
    fn fold_pass(text: &str) -> (String, SpanMap) {
        let mut b = SpanMapBuilder::with_capacity(text);
        let mut buf = String::new();
        for ch in text.chars() {
            if ('\u{ff10}'..='\u{ff19}').contains(&ch) {
                if !buf.is_empty() {
                    b.keep(&buf);
                    buf.clear();
                }
                let folded = char::from(b'0' + (ch as u32 - 0xff10) as u8);
                b.replace(&ch.to_string(), &folded.to_string(), R_FOLD);
            } else {
                buf.push(ch);
            }
        }
        b.keep(&buf);
        b.finish()
    }

    /// 숫자 사이 하이픈을 흡수하는 2번 패스.
    fn separator_pass(text: &str) -> (String, SpanMap) {
        let mut b = SpanMapBuilder::with_capacity(text);
        let chars: Vec<char> = text.chars().collect();
        let mut buf = String::new();
        for (k, ch) in chars.iter().enumerate() {
            let between_digits = *ch == '-'
                && k > 0
                && k + 1 < chars.len()
                && chars[k - 1].is_ascii_digit()
                && chars[k + 1].is_ascii_digit();
            if between_digits {
                if !buf.is_empty() {
                    b.keep(&buf);
                    buf.clear();
                }
                b.absorb("-", R_SEP, Absorbed::Separator);
            } else {
                buf.push(*ch);
            }
        }
        b.keep(&buf);
        b.finish()
    }

    #[test]
    fn compose_with_identity_on_either_side_is_a_noop() {
        let text = "번호 １２-34";
        let (folded, m) = fold_pass(text);
        m.validate().unwrap();

        let left = SpanMap::compose(&SpanMap::identity(text), &m).unwrap();
        left.validate().unwrap();
        assert_eq!(left, m, "항등을 앞에 합성해도 그대로여야 한다");

        let right = SpanMap::compose(&m, &SpanMap::identity(&folded)).unwrap();
        right.validate().unwrap();
        assert_eq!(right, m, "항등을 뒤에 합성해도 그대로여야 한다");
    }

    #[test]
    fn compose_two_passes_recovers_original_offsets() {
        let text = "계좌 １２３４-5678 입금";
        let (after_fold, m1) = fold_pass(text);
        assert_eq!(after_fold, "계좌 1234-5678 입금");
        let (after_sep, m2) = separator_pass(&after_fold);
        assert_eq!(after_sep, "계좌 12345678 입금");

        m1.validate().unwrap();
        m2.validate().unwrap();
        let composed = SpanMap::compose(&m1, &m2).unwrap();
        composed.validate().unwrap();

        // 최종 문자열에서 "12345678" 은 바이트 7..15, 문자 3..11.
        let digits = Span::new(7..15, 3..11);
        assert_eq!(digits.slice(&after_sep), "12345678");

        let got = composed.to_source(&digits);
        // 원문의 "１２３４-5678" 은 전각 4자 12바이트 + 하이픈 1 + 숫자 4 = 17바이트, 문자 9자.
        assert_eq!(got.span.slice(text), "１２３４-5678");
        assert_eq!(got.span.char, 3..12);
        assert!(!got.snapped);
        assert_eq!(got.cost.replaced_chars, 4);
        assert_eq!(got.cost.absorbed_separators, 1);
        assert_eq!(got.rules.len(), 2);
    }

    #[test]
    fn compose_is_associative() {
        let text = "코드 １２-3-４5 끝";
        let (t1, m1) = fold_pass(text);
        let (t2, m2) = separator_pass(&t1);
        let (_t3, m3) = fold_pass(&t2);

        let left = SpanMap::compose(&SpanMap::compose(&m1, &m2).unwrap(), &m3).unwrap();
        let right = SpanMap::compose(&m1, &SpanMap::compose(&m2, &m3).unwrap()).unwrap();
        left.validate().unwrap();
        right.validate().unwrap();
        assert_eq!(left, right, "합성은 결합법칙을 만족해야 한다");
    }

    /// 회귀 테스트. 규칙 목록이 합성 묶는 순서에 따라 달라지던 결함을 막는다.
    ///
    /// 스팬·종류·비용은 원래도 결합법칙을 만족했지만 `rules` 의 **순서**만 달랐다.
    /// 중간 단계에서 세그먼트가 묶이는 굵기가 달라 수집 순서가 뒤집혔기 때문이다.
    /// 이름 오름차순 정규화로 해결했고, 여기서 그 정렬을 직접 확인한다.
    #[test]
    fn rules_are_sorted_canonically() {
        let mut b = SpanMapBuilder::new();
        b.numeral("팔", "8", R_NUM);
        b.replace("１", "1", R_FOLD);
        b.absorb("-", R_SEP, Absorbed::Separator);
        b.keep("9");
        let (_, map) = b.finish();
        map.validate().unwrap();

        let got = map.to_source(&whole(&map));
        let mut expected = vec![R_NUM, R_FOLD, R_SEP];
        expected.sort_unstable();
        assert_eq!(got.rules, expected, "규칙은 적용 순서가 아니라 이름 순으로 나와야 한다");
        assert_ne!(got.rules, vec![R_NUM, R_FOLD, R_SEP], "적용 순서와는 실제로 달라야 의미가 있다");
    }

    #[test]
    fn compose_rejects_mismatched_coordinate_systems() {
        let m1 = SpanMap::identity("abc");
        let m2 = SpanMap::identity("abcdef");
        let err = SpanMap::compose(&m1, &m2).unwrap_err();
        assert!(matches!(err, Error::SpanMapMismatch { .. }));
    }

    #[test]
    fn validate_catches_a_gap() {
        let broken = SpanMap {
            segments: vec![
                Segment {
                    src: Span::new(0..3, 0..3),
                    dst: Span::new(0..3, 0..3),
                    kind: SegmentKind::Identity,
                    rules: Vec::new(),
                    cost: NormalizationCost::default(),
                },
                Segment {
                    // src 가 3 이 아니라 4 에서 시작한다. 빈틈이다.
                    src: Span::new(4..6, 4..6),
                    dst: Span::new(3..5, 3..5),
                    kind: SegmentKind::Identity,
                    rules: Vec::new(),
                    cost: NormalizationCost::default(),
                },
            ],
        };
        let err = broken.validate().unwrap_err();
        assert!(matches!(err, Error::SpanMapInvariant { index: 1, .. }));
    }

    #[test]
    fn validate_catches_identity_carrying_a_rule() {
        let broken = SpanMap {
            segments: vec![Segment {
                src: Span::new(0..3, 0..3),
                dst: Span::new(0..3, 0..3),
                kind: SegmentKind::Identity,
                rules: vec![R_SEP],
                cost: NormalizationCost::default(),
            }],
        };
        assert!(broken.validate().is_err());
    }

    // ── 무작위 입력 불변식 테스트 ───────────────────────────
    //
    // 외부 크레이트에 의존하지 않기 위해(독립성 원칙) 선형 합동 생성기를 직접 쓴다.
    // 시드가 고정이라 실패가 항상 재현된다.

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> usize {
            (self.next() % n) as usize
        }
    }

    const ALPHABET: [&str; 8] = ["a", "1", "-", " ", "가", "팔", "１", "."];

    fn random_text(rng: &mut Lcg, len: usize) -> String {
        (0..len).map(|_| ALPHABET[rng.below(ALPHABET.len() as u64)]).collect()
    }

    fn random_map(rng: &mut Lcg, text: &str) -> (String, SpanMap) {
        let mut b = SpanMapBuilder::with_capacity(text);
        let chars: Vec<char> = text.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let take = (1 + rng.below(3)).min(chars.len() - k);
            let piece: String = chars[k..k + take].iter().collect();
            // 빌더가 낼 수 있는 모든 연산이 여기 있어야 한다. 목록에서 빠진 분기는 한 번도
            // 실행되지 않고, 실행되지 않은 경로의 결함은 실사용에서 처음 드러난다.
            match rng.below(6) {
                0 => b.absorb(&piece, R_SEP, Absorbed::Separator),
                1 => b.absorb(&piece, R_SEP, Absorbed::Whitespace),
                2 => b.replace(&piece, "X", R_FOLD),
                3 => b.numeral(&piece, "123", R_NUM),
                4 => b.numeral(&piece, "1234567", R_NUM),
                _ => b.keep(&piece),
            }
            k += take;
        }
        b.finish()
    }

    #[test]
    fn random_pipelines_keep_every_invariant() {
        let mut rng = Lcg(0x5eed_1234);
        for round in 0..400 {
            let len = 1 + rng.below(24);
            let text = random_text(&mut rng, len);
            let (t1, m1) = random_map(&mut rng, &text);
            let (t2, m2) = random_map(&mut rng, &t1);
            let (_t3, m3) = random_map(&mut rng, &t2);

            m1.validate().unwrap_or_else(|e| panic!("round {round} m1: {e}"));
            m2.validate().unwrap_or_else(|e| panic!("round {round} m2: {e}"));
            m3.validate().unwrap_or_else(|e| panic!("round {round} m3: {e}"));

            // 피복: 합성 결과도 불변식을 지킨다.
            let c12 = SpanMap::compose(&m1, &m2).unwrap_or_else(|e| panic!("round {round} c12: {e}"));
            c12.validate().unwrap_or_else(|e| panic!("round {round} c12 validate: {e}"));

            // 좌표계 보존: 합성 결과의 양끝이 원본 양끝과 같다.
            assert_eq!(c12.src_end(), m1.src_end(), "round {round}");
            assert_eq!(c12.dst_end(), m2.dst_end(), "round {round}");

            // 결합법칙.
            let c23 = SpanMap::compose(&m2, &m3).unwrap_or_else(|e| panic!("round {round} c23: {e}"));
            let left = SpanMap::compose(&c12, &m3).unwrap_or_else(|e| panic!("round {round} left: {e}"));
            let right = SpanMap::compose(&m1, &c23).unwrap_or_else(|e| panic!("round {round} right: {e}"));
            left.validate().unwrap_or_else(|e| panic!("round {round} left validate: {e}"));
            assert_eq!(left, right, "round {round}: 합성 결합법칙");

            // 전체 스팬을 되돌리면 내용이 있는 구간 전체가 나온다.
            // 맨 앞뒤에 붙은 흡수 구간(폭 0)은 경계 규칙에 따라 제외된다.
            let (db, dc) = left.dst_end();
            if db > 0 {
                let got = left.to_source(&Span::new(0..db, 0..dc));
                let first = left.segments().iter().find(|s| !s.dst.is_empty()).unwrap();
                let last = left.segments().iter().rev().find(|s| !s.dst.is_empty()).unwrap();
                assert_eq!(got.span.byte.start, first.src.byte.start, "round {round}");
                assert_eq!(got.span.byte.end, last.src.byte.end, "round {round}");
                assert!(!got.snapped, "round {round}: 전체 스팬은 스냅이 필요 없다");
            }

            // 임의의 부분 스팬도 원문 범위 안에 들어가고 순서가 뒤집히지 않는다.
            if db > 1 {
                let mut lo = rng.below(u64::from(db)) as u32;
                let mut hi = rng.below(u64::from(db)) as u32;
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                // 문자 경계로 맞춘다.
                let lo = char_boundary(&left, lo);
                let hi = char_boundary(&left, hi);
                if let (Some((lb, lc)), Some((hb, hc))) = (lo, hi) {
                    let got = left.to_source(&Span::new(lb..hb, lc..hc));
                    assert!(got.span.byte.start <= got.span.byte.end, "round {round}");
                    assert!(got.span.byte.end <= left.src_end().0, "round {round}");
                    assert!(got.span.char.end <= left.src_end().1, "round {round}");
                }
            }
        }
    }

    /// 세그먼트 경계에서 (바이트, 문자) 쌍을 찾는다. 무작위 테스트가 문자 경계를 지키게 한다.
    fn char_boundary(map: &SpanMap, target_byte: u32) -> Option<(u32, u32)> {
        for seg in map.segments() {
            if seg.dst.byte.start >= target_byte {
                return Some((seg.dst.byte.start, seg.dst.char.start));
            }
            if seg.dst.byte.end >= target_byte {
                return Some((seg.dst.byte.end, seg.dst.char.end));
            }
        }
        Some(map.dst_end())
    }
}
