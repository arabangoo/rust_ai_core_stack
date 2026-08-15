//! 문맥 단서 사전과 점수.
//!
//! 계좌번호처럼 숫자열만으로는 판별이 불가능한 엔티티에 필수다. `1234567890` 이 계좌번호인지
//! 주문번호인지는 규칙으로 알 수 없고, 주변에 `입금` 이나 `예금주` 가 있는지가 유일한 신호다.
//!
//! 단서는 스팬 주변의 설정된 창 안에서만 찾고, **거리에 따라 가중치를 감쇠**시킨다.
//! 바로 앞에 붙은 `주민등록번호` 와 두 문장 건너의 그것은 같은 무게일 수 없다.

use serde::Serialize;

use crate::span::Span;

/// 문맥 단서 하나. 단어와 그 단어가 갖는 기본 무게다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cue {
    /// 찾을 단어.
    pub word: &'static str,
    /// 이 단어가 붙어 있을 때의 기본 가산 무게.
    pub weight: f32,
}

/// 실제로 잡힌 단서. 판정 근거에 그대로 실린다.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ContextHit {
    /// 잡힌 단어.
    pub cue: &'static str,
    /// 스팬 경계로부터의 문자 거리.
    pub distance: u32,
    /// 거리 감쇠까지 반영한 실제 가산 무게.
    pub weight: f32,
}

const fn cue(word: &'static str, weight: f32) -> Cue {
    Cue { word, weight }
}

/// 주민등록번호 단서.
pub const RESIDENT: &[Cue] = &[
    cue("주민등록번호", 1.0),
    cue("주민번호", 1.0),
    cue("주민등록", 0.8),
    cue("본인확인", 0.4),
    cue("실명확인", 0.4),
];

/// 외국인등록번호 단서.
pub const FOREIGNER: &[Cue] = &[
    cue("외국인등록번호", 1.0),
    cue("외국인등록", 0.8),
    cue("외국인", 0.5),
    cue("체류", 0.4),
];

/// 사업자등록번호 단서.
pub const BUSINESS: &[Cue] = &[
    cue("사업자등록번호", 1.0),
    cue("사업자번호", 1.0),
    cue("사업자", 0.6),
    cue("세금계산서", 0.5),
    cue("법인", 0.3),
    cue("상호", 0.3),
];

/// 카드번호 단서.
pub const CARD: &[Cue] = &[
    cue("카드번호", 1.0),
    cue("신용카드", 1.0),
    cue("체크카드", 1.0),
    cue("유효기간", 0.5),
    cue("카드", 0.6),
    cue("결제", 0.4),
    cue("승인", 0.3),
];

/// 계좌번호 단서. 오탐 위험이 가장 큰 엔티티라 사전이 가장 두껍다.
pub const ACCOUNT: &[Cue] = &[
    cue("계좌번호", 1.0),
    cue("예금주", 0.9),
    cue("계좌", 0.8),
    cue("입금", 0.7),
    cue("이체", 0.7),
    cue("송금", 0.7),
    cue("은행", 0.6),
    cue("환불", 0.4),
];

/// 전화번호 단서.
pub const PHONE: &[Cue] = &[
    cue("전화번호", 1.0),
    cue("휴대전화", 1.0),
    cue("휴대폰", 1.0),
    cue("핸드폰", 1.0),
    cue("연락처", 1.0),
    cue("전화", 0.6),
    cue("문자", 0.4),
    cue("통화", 0.3),
];

/// 운전면허번호 단서.
pub const DRIVER_LICENSE: &[Cue] = &[
    cue("운전면허번호", 1.0),
    cue("운전면허", 1.0),
    cue("면허번호", 0.9),
    cue("면허증", 0.8),
];

/// 여권번호 단서.
pub const PASSPORT: &[Cue] = &[cue("여권번호", 1.0), cue("여권", 0.8)];

/// 생년월일 단서.
pub const BIRTH_DATE: &[Cue] = &[
    cue("생년월일", 1.0),
    cue("출생일", 1.0),
    cue("생일", 0.8),
    cue("출생", 0.6),
    cue("년생", 0.6),
];

/// 부정 문맥 사전. **이 숫자는 개인의 것이 아니다**라고 말하는 낱말들이다.
///
/// 지금까지의 단서는 전부 "이건 개인정보다" 쪽으로만 밀었다. 그래서 업무 문서가 통째로
/// 오탐이 된다. 계약서 한 장에는 계약번호·증권번호·접수번호가 스무 개씩 들어 있고, 그중
/// 열 자리짜리는 사업자등록번호 후보가 되며 열한 자리짜리는 전화번호 후보가 된다.
///
/// ## 검증식은 이 사전을 이긴다
///
/// 부정 단서가 있어도 **검증식을 통과한 값은 살아남는다.** `증권번호` 옆에 있더라도 주민등록
/// 검사 숫자가 맞는 열세 자리는 잘못 붙은 이름표일 뿐 개인정보다. 부정 문맥은 검증식이 없는
/// 엔티티에서만 결정력을 갖는다 — 그쪽이 원래 오탐이 나는 자리다.
///
/// ## 긍정 단서보다 무거울 때만 이긴다
///
/// `계약번호 2026-0001 담당자 연락처 010-1234-5678` 처럼 두 종류가 한 창에 같이 들어오는
/// 문장이 흔하다. 부정 단서가 보이기만 하면 버리는 규칙은 이 문장의 전화번호를 놓친다.
/// 그래서 양쪽 총점을 견줘 **부정이 더 무거울 때만** 버린다.
///
/// 콜센터·대표번호가 들어 있는 이유는 다르다. 그것은 진짜 전화번호지만 **개인의 것이 아니다.**
/// 회사 안내문 꼬리마다 붙는 번호를 개인정보로 세면 마스킹 결과물이 읽을 수 없게 된다.
pub const EXCLUDE: &[Cue] = &[
    // 문서·거래 식별자
    cue("증권번호", 1.0),
    cue("계약번호", 1.0),
    cue("접수번호", 1.0),
    cue("주문번호", 1.0),
    cue("송장번호", 1.0),
    cue("운송장", 0.8),
    cue("관리번호", 0.9),
    cue("문서번호", 0.9),
    cue("일련번호", 0.9),
    cue("발급번호", 0.8),
    cue("전표번호", 0.8),
    // 물건 식별자
    cue("상품코드", 1.0),
    cue("제품코드", 1.0),
    cue("모델번호", 1.0),
    cue("모델명", 0.8),
    cue("품번", 0.8),
    cue("바코드", 0.8),
    // 개인의 것이 아닌 전화번호
    cue("콜센터", 1.0),
    cue("고객센터", 1.0),
    cue("대표번호", 1.0),
    cue("상담센터", 0.9),
    cue("ARS", 0.8),
    // 법령·문서 구조
    cue("우편번호", 1.0),
    cue("별표", 0.6),
    cue("고시", 0.5),
];

/// `byte_at` 에서 앞으로 `chars` 문자만큼의 구간을 낸다.
fn window_before(text: &str, byte_at: usize, chars: u32) -> &str {
    let mut start = byte_at;
    for (index, (byte_index, _)) in text[..byte_at].char_indices().rev().enumerate() {
        start = byte_index;
        if index as u32 + 1 >= chars {
            break;
        }
    }
    &text[start..byte_at]
}

/// `byte_at` 에서 뒤로 `chars` 문자만큼의 구간을 낸다.
fn window_after(text: &str, byte_at: usize, chars: u32) -> &str {
    let rest = &text[byte_at..];
    let mut end = rest.len();
    for (index, (byte_index, c)) in rest.char_indices().enumerate() {
        if index as u32 >= chars {
            end = byte_index;
            break;
        }
        let _ = c;
    }
    &rest[..end]
}

/// 거리에 따른 감쇠 계수. 붙어 있으면 1.0, 멀어질수록 완만히 준다.
fn decay(distance: u32) -> f32 {
    1.0 / (1.0 + distance as f32 / 8.0)
}

/// 창 안에서 단서를 찾는다. 같은 단어가 여러 번 나오면 **가장 가까운 것 하나**만 남긴다.
///
/// 반환 목록은 무게 내림차순으로 정렬되고, 무게가 같으면 단어 오름차순이다. 정렬을 고정하는
/// 이유는 결정성 때문이다. 같은 입력이면 판정 근거도 글자 단위로 같아야 한다.
pub fn find(text: &str, span: &Span, window: u32, cues: &[Cue]) -> Vec<ContextHit> {
    let before = window_before(text, span.byte.start as usize, window);
    let after = window_after(text, span.byte.end as usize, window);

    let mut hits: Vec<ContextHit> = Vec::new();
    for cue in cues {
        let mut best: Option<u32> = None;

        // 앞쪽. 단서의 끝에서 스팬 시작까지의 거리를 잰다.
        if let Some(byte_index) = before.rfind(cue.word) {
            let tail = &before[byte_index + cue.word.len()..];
            best = Some(tail.chars().count() as u32);
        }
        // 뒤쪽. 스팬 끝에서 단서의 시작까지의 거리를 잰다.
        if let Some(byte_index) = after.find(cue.word) {
            let head = &after[..byte_index];
            let distance = head.chars().count() as u32;
            best = Some(best.map_or(distance, |b| b.min(distance)));
        }

        if let Some(distance) = best {
            hits.push(ContextHit {
                cue: cue.word,
                distance,
                weight: cue.weight * decay(distance),
            });
        }
    }

    hits.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cue.cmp(b.cue))
    });
    hits
}

/// 단서 목록의 총 가산점. 겹치는 단서가 많다고 무한정 오르지 않게 상한을 둔다.
///
/// 상한이 없으면 `주민등록번호` 와 `주민번호` 와 `주민등록` 이 한꺼번에 잡히는 문장에서
/// 점수가 세 배로 뛴다. 세 단어가 같은 사실을 세 번 말하는 것이지 근거가 세 배인 것은 아니다.
pub fn total(hits: &[ContextHit]) -> f32 {
    hits.iter().map(|hit| hit.weight).sum::<f32>().min(1.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 텍스트에서 부분 문자열의 스팬을 만든다.
    fn span_of(text: &str, needle: &str) -> Span {
        let byte_start = text.find(needle).expect("부분 문자열이 없다");
        let byte_end = byte_start + needle.len();
        let char_start = text[..byte_start].chars().count() as u32;
        let char_end = char_start + needle.chars().count() as u32;
        Span::new(byte_start as u32..byte_end as u32, char_start..char_end)
    }

    #[test]
    fn finds_cue_before_the_span() {
        let text = "주민등록번호 8801011234567 입니다";
        let hits = find(text, &span_of(text, "8801011234567"), 20, RESIDENT);
        assert_eq!(hits[0].cue, "주민등록번호");
        assert_eq!(hits[0].distance, 1, "공백 한 칸");
    }

    #[test]
    fn finds_cue_after_the_span() {
        let text = "8801011234567 은 주민번호 입니다";
        let hits = find(text, &span_of(text, "8801011234567"), 20, RESIDENT);
        let hit = hits.iter().find(|h| h.cue == "주민번호").unwrap();
        assert_eq!(hit.distance, 3, "공백, 은, 공백");
    }

    #[test]
    fn nearer_cue_scores_higher() {
        let near = "계좌번호 1234567890";
        let far = "계좌번호 그리고 아주 긴 설명이 이어진 다음에 나오는 1234567890";
        let near_hits = find(near, &span_of(near, "1234567890"), 40, ACCOUNT);
        let far_hits = find(far, &span_of(far, "1234567890"), 40, ACCOUNT);
        assert!(
            total(&near_hits) > total(&far_hits),
            "가까운 단서가 더 무거워야 한다"
        );
    }

    #[test]
    fn window_limits_the_search() {
        let text = "계좌번호 그리고 여기에 아주 긴 문장이 끼어 있어서 창을 벗어난다 1234567890";
        let span = span_of(text, "1234567890");
        assert!(find(text, &span, 5, ACCOUNT).is_empty(), "창 밖은 안 잡힌다");
        assert!(!find(text, &span, 60, ACCOUNT).is_empty(), "창을 넓히면 잡힌다");
    }

    #[test]
    fn no_cue_yields_no_hits() {
        let text = "주문번호 1234567890 로 접수되었습니다";
        assert!(find(text, &span_of(text, "1234567890"), 20, ACCOUNT).is_empty());
    }

    #[test]
    fn total_is_capped() {
        // 같은 사실을 여러 단어로 말해도 상한을 넘지 않는다.
        let text = "주민등록번호 주민번호 주민등록 본인확인 실명확인 8801011234567";
        let hits = find(text, &span_of(text, "8801011234567"), 60, RESIDENT);
        assert!(hits.len() >= 4, "여러 단서가 잡혀야 하는 문장이다");
        assert!(total(&hits) <= 1.5);
    }

    #[test]
    fn hits_are_sorted_deterministically() {
        let text = "계좌번호 예금주 입금 1234567890";
        let hits = find(text, &span_of(text, "1234567890"), 40, ACCOUNT);
        for pair in hits.windows(2) {
            assert!(
                pair[0].weight > pair[1].weight
                    || (pair[0].weight == pair[1].weight && pair[0].cue < pair[1].cue),
                "무게 내림차순, 동률이면 단어 오름차순이어야 한다"
            );
        }
    }

    #[test]
    fn negative_cues_are_found_the_same_way() {
        let text = "증권번호 1234567890 로 조회하세요";
        let hits = find(text, &span_of(text, "1234567890"), 20, EXCLUDE);
        assert_eq!(hits[0].cue, "증권번호");
    }

    #[test]
    fn the_two_dictionaries_do_not_overlap() {
        // 같은 낱말이 양쪽에 있으면 자기 자신과 견주게 되어 판정이 무의미해진다.
        let positives = [
            RESIDENT,
            FOREIGNER,
            BUSINESS,
            CARD,
            ACCOUNT,
            PHONE,
            DRIVER_LICENSE,
            PASSPORT,
            BIRTH_DATE,
        ];
        for list in positives {
            for cue in list {
                assert!(
                    !EXCLUDE.iter().any(|bad| bad.word == cue.word),
                    "단서가 양쪽 사전에 다 있다: {}",
                    cue.word
                );
            }
        }
    }

    #[test]
    fn window_handles_text_boundaries() {
        let text = "8801011234567";
        let span = span_of(text, "8801011234567");
        assert!(find(text, &span, 20, RESIDENT).is_empty());
    }

    #[test]
    fn windows_respect_char_boundaries() {
        // 한글은 한 글자가 3바이트다. 창 계산이 바이트로 새면 여기서 패닉한다.
        let text = "가나다라마바사아자차 8801011234567 가나다라마바사아자차";
        let span = span_of(text, "8801011234567");
        let _ = find(text, &span, 5, RESIDENT);
        let _ = find(text, &span, 1, RESIDENT);
        let _ = find(text, &span, 100, RESIDENT);
    }
}
