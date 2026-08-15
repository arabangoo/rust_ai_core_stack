//! 1번 패스: 한글 자모 조합.
//!
//! macOS 계열에서 만들어진 텍스트와 일부 입력기 출력은 한글을 **자모 분리형**으로 담는다.
//! `한` 이 `ᄒ`(U+1112) `ᅡ`(U+1161) `ᆫ`(U+11AB) 세 코드포인트로 들어오는 식이다.
//! 이 패스를 넣지 않으면 그런 입력에서 조용히 미탐이 난다.
//!
//! ## 왜 전체 정규화 형식 C 가 아닌가
//!
//! 한글 조합은 **순수 산술식**이라 표가 필요 없다. 조합형 자모의 코드포인트에서
//! 음절 코드포인트를 바로 계산할 수 있다(유니코드 표준 3.12 의 한글 조합 알고리즘).
//! 반면 전체 정규화 형식 C 를 하려면 유니코드 합성 표 전체를 들여와야 하고,
//! 그것은 기본 빌드의 의존성 0 원칙(README 16절)을 깬다.
//!
//! **그래서 이 패스는 한글만 조합한다.** 라틴 문자의 분해형(`e` + 결합 악센트)이나
//! 다른 문자 체계의 분해형은 그대로 통과한다. 한국어 개인정보 탐지라는 이 라이브러리의
//! 용도에서 그 범위가 실제로 문제가 되는 입력은 관측되지 않았고, 문제가 되는 날이 오면
//! 그때 선택적 기능으로 전체 정규화를 붙인다.
//!
//! 호환용 자모 블록(U+3131 이상의 `ㄱ` `ㄴ`)은 **조합하지 않는다.** 표준 정규화 형식 C 도
//! 그것을 조합하지 않으며, 그 블록은 낱자 자체를 뜻하는 별개의 문자다.

use crate::span::{RuleId, SpanMap, SpanMapBuilder};

/// 분리형 자모를 음절로 합쳤음을 뜻하는 규칙 이름.
pub const RULE_JAMO: RuleId = "nfc.hangul_jamo";

const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
/// 종성 없음을 0 으로 두기 위해 실제 종성 시작(U+11A8)보다 1 작다.
const T_BASE: u32 = 0x11A7;
const S_BASE: u32 = 0xAC00;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const S_COUNT: u32 = 19 * V_COUNT * T_COUNT;

fn is_lead(c: char) -> bool {
    ('\u{1100}'..='\u{1112}').contains(&c)
}

fn is_vowel(c: char) -> bool {
    ('\u{1161}'..='\u{1175}').contains(&c)
}

fn is_trail(c: char) -> bool {
    ('\u{11A8}'..='\u{11C2}').contains(&c)
}

/// 종성이 없는 완성 음절인가. 여기에 종성을 붙일 수 있다.
fn is_lv_syllable(c: char) -> bool {
    let s = c as u32;
    (S_BASE..S_BASE + S_COUNT).contains(&s) && (s - S_BASE) % T_COUNT == 0
}

/// 조합 대상 코드포인트가 하나라도 있는가. 없으면 항등 매핑으로 끝낸다.
fn has_conjoining_jamo(text: &str) -> bool {
    text.chars().any(|c| is_lead(c) || is_vowel(c) || is_trail(c))
}

/// `cs[i]` 에서 시작하는 조합 가능한 자모열을 음절 하나로 합친다.
///
/// 반환값은 합쳐진 음절과 **소비한 문자 수**다.
fn compose_at(cs: &[(usize, char)], i: usize) -> Option<(char, usize)> {
    let (_, c0) = cs[i];

    // 초성 + 중성 (+ 종성)
    if is_lead(c0) {
        let &(_, c1) = cs.get(i + 1)?;
        if !is_vowel(c1) {
            return None;
        }
        let l = c0 as u32 - L_BASE;
        let v = c1 as u32 - V_BASE;
        let mut t = 0;
        let mut consumed = 2;
        if let Some(&(_, c2)) = cs.get(i + 2) {
            if is_trail(c2) {
                t = c2 as u32 - T_BASE;
                consumed = 3;
            }
        }
        let s = S_BASE + (l * V_COUNT + v) * T_COUNT + t;
        return char::from_u32(s).map(|c| (c, consumed));
    }

    // 완성 음절(종성 없음) + 종성
    if is_lv_syllable(c0) {
        let &(_, c1) = cs.get(i + 1)?;
        if is_trail(c1) {
            let s = c0 as u32 + (c1 as u32 - T_BASE);
            return char::from_u32(s).map(|c| (c, 2));
        }
    }

    None
}

/// 자모 조합 패스를 적용한다.
pub fn apply(text: &str) -> (String, SpanMap) {
    if !has_conjoining_jamo(text) {
        return (text.to_string(), SpanMap::identity(text));
    }

    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let mut builder = SpanMapBuilder::with_capacity(text);
    let mut flushed = 0;
    let mut i = 0;

    while i < cs.len() {
        let (start, _) = cs[i];
        match compose_at(&cs, i) {
            Some((syllable, consumed)) => {
                if flushed < start {
                    builder.keep(&text[flushed..start]);
                }
                let end = cs
                    .get(i + consumed)
                    .map_or(text.len(), |&(byte, _)| byte);
                let mut buf = [0u8; 4];
                builder.replace(&text[start..end], syllable.encode_utf8(&mut buf), RULE_JAMO);
                flushed = end;
                i += consumed;
            }
            None => i += 1,
        }
    }

    if flushed < text.len() {
        builder.keep(&text[flushed..]);
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_lead_vowel_trail() {
        let (out, _) = apply("\u{1112}\u{1161}\u{11AB}");
        assert_eq!(out, "한");
    }

    #[test]
    fn composes_lead_vowel_without_trail() {
        let (out, _) = apply("\u{1112}\u{1161}");
        assert_eq!(out, "하");
    }

    #[test]
    fn attaches_trail_to_precomposed_syllable() {
        let (out, _) = apply("하\u{11AB}");
        assert_eq!(out, "한");
    }

    #[test]
    fn leaves_precomposed_text_untouched() {
        let (out, map) = apply("한글 880101");
        assert_eq!(out, "한글 880101");
        assert!(map.is_identity());
    }

    #[test]
    fn leaves_compatibility_jamo_alone() {
        // U+3131 은 낱자 자체를 뜻하는 별개 문자다. 표준 정규화 형식 C 도 조합하지 않는다.
        let (out, map) = apply("\u{3131}\u{3161}");
        assert_eq!(out, "\u{3131}\u{3161}");
        assert!(map.is_identity());
    }

    #[test]
    fn leaves_dangling_lead_alone() {
        let (out, _) = apply("\u{1112}김");
        assert_eq!(out, "\u{1112}김");
    }

    #[test]
    fn map_recovers_source_bytes() {
        let src = "\u{1112}\u{1161}\u{11AB}글";
        let (out, map) = apply(src);
        assert_eq!(out, "한글");
        map.validate().unwrap();
        // 정규화문의 '한' 한 글자가 원문에서는 자모 세 개(9바이트)다.
        let recovered = map.to_source(&crate::span::Span::new(0..3, 0..1));
        assert_eq!(recovered.span.byte, 0..9);
        assert_eq!(recovered.rules, vec![RULE_JAMO]);
    }
}
