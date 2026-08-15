//! 4번 패스: 숫자 사이에 낀 유사문자를 숫자로 되돌린다.
//!
//! `88O1O1` 의 `O` 는 영문자지만 숫자 `0` 을 대신해 적힌 것이다. 사람이 눈으로는 읽지만
//! 숫자 스캐너는 런을 세 조각으로 끊어 아무것도 못 잡는다. 다국어 개인정보 벤치마크가
//! 프론티어 모델의 실패 모드로 지목한 **문자 치환(character substitution)** 이 이 자리다.
//!
//! ## 안쪽에서만 바꾼다
//!
//! 이 변환은 위험하다. `O` 를 무조건 `0` 으로 바꾸면 `POLO` 가 `P0L0` 이 되어 정상 텍스트를
//! 훼손하고 없던 숫자열을 만든다. 그래서 **양옆이 모두 숫자 자리일 때만** 바꾼다.
//!
//! | 입력 | 결과 | 왜 |
//! | --- | --- | --- |
//! | `88O1O1` | `880101` | `O` 양옆이 숫자다 |
//! | `POLO` | `POLO` | 왼쪽이 `P` 다 |
//! | `50l` | `50l` | 오른쪽이 없다 |
//! | `88-O1` | `880 1` 아님 | 붙임표는 건너뛰고 `8` 을 본다. 바꾼다 |
//!
//! 마지막 줄이 요점이다. 이 패스는 [`super::separator`] 보다 **먼저** 돌기 때문에 구분자가
//! 아직 남아 있다. 그래서 양옆을 볼 때 구분자를 건너뛴다. 그러지 않으면 `88-O1-O1` 처럼
//! 구분자와 유사문자가 섞인 실제 사례를 놓친다.
//!
//! ## 문맥을 보는 패스다
//!
//! 양옆을 보므로 조각으로 잘라 정규화하면 결과가 달라진다. [`super::NormalizeConfig::context_free`]
//! 에 이 패스가 없는 이유이고, 조각 단위 왕복 불변식의 대상이 아닌 이유이기도 하다.

use crate::span::{RuleId, SpanMap, SpanMapBuilder};

/// 유사문자를 숫자로 되돌렸다.
pub const RULE_LOOKALIKE: RuleId = "lookalike.digit";

/// 이 문자가 대신하고 있을 수 있는 숫자.
///
/// 보수적으로 넷만 둔다. `S`→`5`, `B`→`8` 같은 것은 정상 텍스트에서 너무 흔해 넣지 않는다.
fn lookalike_digit(c: char) -> Option<char> {
    match c {
        'O' | 'o' => Some('0'),
        'l' | 'I' => Some('1'),
        _ => None,
    }
}

/// 숫자 자리로 볼 수 있는 문자인가. 마스킹 문자도 숫자 자리다.
fn is_digit_like(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '*' | 'X' | 'x')
}

/// 양옆을 볼 때 건너뛰는 문자인가. 구분자 흡수 패스가 나중에 없앨 것들이다.
fn is_skippable(c: char) -> bool {
    matches!(c, '-' | '.' | ',' | '(' | ')' | '·' | '・') || c == ' ' || c == '\t'
}

/// `chars[at]` 의 한쪽이 숫자 자리인가. 구분자는 건너뛴다.
fn side_is_digit(chars: &[char], at: usize, forward: bool) -> bool {
    let mut index = at;
    loop {
        index = if forward {
            match index + 1 {
                next if next < chars.len() => next,
                _ => return false,
            }
        } else {
            match index.checked_sub(1) {
                Some(prev) => prev,
                None => return false,
            }
        };
        let c = chars[index];
        if is_digit_like(c) {
            return true;
        }
        if !is_skippable(c) {
            return false;
        }
    }
}

/// 유사문자 교정 패스를 적용한다.
pub fn apply(text: &str) -> (String, SpanMap) {
    if !text.chars().any(|c| lookalike_digit(c).is_some()) {
        return (text.to_string(), SpanMap::identity(text));
    }

    let chars: Vec<char> = text.chars().collect();
    let mut builder = SpanMapBuilder::with_capacity(text);
    let mut buffered = String::new();

    for (index, &c) in chars.iter().enumerate() {
        let replacement = lookalike_digit(c).filter(|_| {
            side_is_digit(&chars, index, false) && side_is_digit(&chars, index, true)
        });

        match replacement {
            Some(digit) => {
                if !buffered.is_empty() {
                    builder.keep(&buffered);
                    buffered.clear();
                }
                let mut source = String::new();
                source.push(c);
                let mut target = String::new();
                target.push(digit);
                builder.replace(&source, &target, RULE_LOOKALIKE);
            }
            None => buffered.push(c),
        }
    }

    if !buffered.is_empty() {
        builder.keep(&buffered);
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(text: &str) -> String {
        apply(text).0
    }

    #[test]
    fn converts_lookalikes_between_digits() {
        assert_eq!(out("88O1O1"), "880101");
        assert_eq!(out("4l11"), "4111");
        assert_eq!(out("2I34"), "2134");
    }

    #[test]
    fn skips_separators_when_looking_at_both_sides() {
        // 구분자 흡수보다 먼저 도는 패스라 붙임표가 아직 남아 있다.
        assert_eq!(out("88-O1-O1"), "88-01-01");
        assert_eq!(out("1234 O567"), "1234 0567");
    }

    #[test]
    fn leaves_ordinary_words_alone() {
        for text in ["POLO", "Hello World", "모델 O 타입", "50l", "l50", "OO"] {
            assert_eq!(out(text), text, "정상 텍스트를 훼손했다: {text}");
        }
    }

    #[test]
    fn a_lookalike_at_either_edge_is_untouched() {
        assert_eq!(out("O123"), "O123", "왼쪽이 없다");
        assert_eq!(out("123O"), "123O", "오른쪽이 없다");
    }

    #[test]
    fn masking_characters_count_as_digit_positions() {
        assert_eq!(out("12*O*5"), "12*0*5");
    }

    #[test]
    fn the_mapping_records_the_cost_and_rule() {
        let (text, map) = apply("88O1O1");
        map.validate().unwrap();
        assert_eq!(text, "880101");

        let src = map.to_source(&crate::span::Span::new(0..6, 0..6));
        assert_eq!(src.span.byte, 0..6);
        assert_eq!(src.rules, vec![RULE_LOOKALIKE]);
        assert_eq!(src.cost.replaced_chars, 2);
    }

    #[test]
    fn text_without_lookalikes_takes_the_fast_path() {
        let (text, map) = apply("880101");
        assert_eq!(text, "880101");
        assert!(map.is_identity());
    }
}
