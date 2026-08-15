//! 2번 패스: 전각 폴딩과 유사문자 통일.
//!
//! 전각 숫자 `８８０１０１` 과 반각 숫자 `880101` 은 사람 눈에 같은 값이지만 코드포인트가
//! 다르다. 탐지 층이 숫자 런을 스캔하려면 먼저 한 표기로 모아야 한다.
//!
//! ## 하지 않는 것: 문자와 숫자 사이의 유사문자 교정
//!
//! 대문자 `O` 를 `0` 으로, 소문자 `l` 을 `1` 로 바꾸는 교정은 **하지 않는다.**
//! 그렇게 하면 `POLO` 가 `P0L0` 이 되고 `hello` 가 `he11o` 가 된다. 정상 텍스트를 훼손하는
//! 데다, 없던 숫자열을 만들어 이 라이브러리가 줄이려는 오탐을 오히려 늘린다.
//! 이 패스가 다루는 것은 **같은 글자의 다른 표기**뿐이다. 전각과 반각, 여러 종류의 대시,
//! 여러 종류의 공백이 거기 해당한다.
//!
//! ## 세그먼트를 문자 단위로 끊는 이유
//!
//! 연속한 전각 숫자를 한 세그먼트로 묶으면 조회 비용은 줄지만, 그 묶음 안쪽을 부분적으로
//! 가리키는 탐지 스팬이 경계 스냅을 유발한다. 이 패스의 변환은 언제나 문자 1개 대 1개라
//! 문자 단위로 끊어 두면 어떤 스팬도 세그먼트 경계에 정확히 떨어진다. 스냅이 붙지 않는
//! 쪽이 판정 근거를 깨끗하게 만든다.

use crate::span::{RuleId, SpanMap, SpanMapBuilder};

/// 전각 영숫자와 기호를 반각으로 바꿨음을 뜻한다.
pub const RULE_FULLWIDTH: RuleId = "fold.fullwidth";
/// 폭이 다른 공백을 보통 공백으로 바꿨음을 뜻한다.
pub const RULE_SPACE: RuleId = "fold.space";
/// 여러 종류의 대시를 붙임표로 바꿨음을 뜻한다.
pub const RULE_DASH: RuleId = "fold.dash";

/// 이 문자를 어떤 문자로 접을 것인가. 접을 필요가 없으면 `None`.
fn fold_char(c: char) -> Option<(char, RuleId)> {
    match c {
        // 전각 ASCII 블록. 전각 붙임표(U+FF0D)와 전각 골뱅이(U+FF20)도 여기서 처리된다.
        '\u{FF01}'..='\u{FF5E}' => {
            char::from_u32(c as u32 - 0xFEE0).map(|folded| (folded, RULE_FULLWIDTH))
        }
        // 폭이 다른 공백들. 표 조판이나 웹 복사에서 흔히 섞여 들어온다.
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => {
            Some((' ', RULE_SPACE))
        }
        // 대시 계열. 문서 편집기가 붙임표를 자동으로 바꿔 놓는 경우가 많다.
        '\u{00AD}' | '\u{2010}'..='\u{2015}' | '\u{2043}' | '\u{2212}' | '\u{FE58}'
        | '\u{FE63}' => Some(('-', RULE_DASH)),
        _ => None,
    }
}

/// 전각 폴딩 패스를 적용한다.
pub fn apply(text: &str) -> (String, SpanMap) {
    if !text.chars().any(|c| fold_char(c).is_some()) {
        return (text.to_string(), SpanMap::identity(text));
    }

    let mut builder = SpanMapBuilder::with_capacity(text);
    let mut flushed = 0;

    for (start, c) in text.char_indices() {
        let Some((folded, rule)) = fold_char(c) else {
            continue;
        };
        if flushed < start {
            builder.keep(&text[flushed..start]);
        }
        let end = start + c.len_utf8();
        let mut buf = [0u8; 4];
        builder.replace(&text[start..end], folded.encode_utf8(&mut buf), rule);
        flushed = end;
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
    fn folds_fullwidth_digits() {
        let (out, _) = apply("８８０１０１");
        assert_eq!(out, "880101");
    }

    #[test]
    fn folds_fullwidth_hyphen_and_at_sign() {
        let (out, _) = apply("８８０１０１－１");
        assert_eq!(out, "880101-1");
        let (out, _) = apply("kim＠example.com");
        assert_eq!(out, "kim@example.com");
    }

    #[test]
    fn folds_space_variants() {
        let (out, _) = apply("880101\u{3000}1234567");
        assert_eq!(out, "880101 1234567");
    }

    #[test]
    fn folds_dash_variants() {
        let (out, _) = apply("880101\u{2013}1234567");
        assert_eq!(out, "880101-1234567");
    }

    #[test]
    fn leaves_letter_digit_confusables_alone() {
        // 유사문자 교정을 하지 않는다는 결정의 회귀 방지 테스트다.
        let (out, map) = apply("POLO hello");
        assert_eq!(out, "POLO hello");
        assert!(map.is_identity());
    }

    #[test]
    fn maps_every_folded_char_without_snapping() {
        let src = "８８０１０１";
        let (out, map) = apply(src);
        map.validate().unwrap();
        // 정규화문 첫 두 글자(반각 2바이트)는 원문에서 전각 2글자(6바이트)다.
        let recovered = map.to_source(&crate::span::Span::new(0..2, 0..2));
        assert_eq!(out, "880101");
        assert_eq!(recovered.span.byte, 0..6);
        assert!(!recovered.snapped, "문자 대 문자 변환이라 스냅이 붙으면 안 된다");
    }
}
