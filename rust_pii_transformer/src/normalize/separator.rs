//! 5번 패스: 숫자 사이 구분자 흡수.
//!
//! `880101 - 1234567` 과 `8801011234567` 을 같은 값으로 보게 만든다. 이 패스가 끝나면
//! 탐지 층은 순수 숫자 런만 상대하면 되고, 그래서 정규 표현식 없이 길이 분류만으로
//! 대부분의 엔티티를 가려낼 수 있다.
//!
//! ## 왜 숫자와 숫자 사이에서만인가
//!
//! 공백을 전역으로 지우면 `제품 1234 수량 5678` 이 `제품12345678` 이 되어 **없던 여덟 자리
//! 숫자열이 생긴다.** 그래서 흡수 조건을 양쪽이 모두 숫자인 경우로 좁힌다.
//!
//! 그래도 위험은 남는다. 흡수한 공백 개수가 [`NormalizationCost::absorbed_whitespace`]
//! 에 기록되어 탐지 층이 신뢰도에서 차감한다. 하이픈 흡수는 위험이 훨씬 낮으므로 별도
//! 항목으로 세고 감점 계수도 다르다.
//!
//! ## 줄바꿈은 흡수하지 않는다
//!
//! 줄이 다르면 다른 값일 확률이 높다. 표의 마지막 열과 다음 행 첫 열이 이어 붙는 사고를
//! 막기 위해 가로 공백(공백, 탭)만 흡수 대상으로 둔다.
//!
//! ## 남은 위험: 소수점
//!
//! `1.5` 가 `15` 가 된다. 설계 문서가 점을 흡수 대상으로 지정했고 전화번호 표기
//! (`010.1234.5678`)에서 실익이 크기 때문에 그대로 따르되, 이 변환도 구분자 흡수로 계수되어
//! 신뢰도에서 차감된다. 소수가 많은 문서를 다룰 때는 이 패스를 끄는 선택지가 있다.
//!
//! [`NormalizationCost::absorbed_whitespace`]: crate::span::NormalizationCost::absorbed_whitespace

use crate::span::{Absorbed, RuleId, SpanMap, SpanMapBuilder};

/// 가로 공백을 흡수했음을 뜻한다. 가장 위험한 흡수라 따로 센다.
pub const RULE_WHITESPACE: RuleId = "separator.whitespace";
/// 붙임표를 흡수했음을 뜻한다.
pub const RULE_HYPHEN: RuleId = "separator.hyphen";
/// 마침표를 흡수했음을 뜻한다.
pub const RULE_DOT: RuleId = "separator.dot";
/// 괄호를 흡수했음을 뜻한다.
pub const RULE_PAREN: RuleId = "separator.paren";
/// 쉼표를 흡수했음을 뜻한다.
pub const RULE_COMMA: RuleId = "separator.comma";
/// 가운뎃점을 흡수했음을 뜻한다.
pub const RULE_MIDDOT: RuleId = "separator.middot";

/// 흡수 후보인가. 후보면 규칙 이름과 감점 분류를 낸다.
fn separator_class(c: char) -> Option<(RuleId, Absorbed)> {
    match c {
        ' ' | '\t' => Some((RULE_WHITESPACE, Absorbed::Whitespace)),
        '-' => Some((RULE_HYPHEN, Absorbed::Separator)),
        '.' => Some((RULE_DOT, Absorbed::Separator)),
        '(' | ')' => Some((RULE_PAREN, Absorbed::Separator)),
        ',' => Some((RULE_COMMA, Absorbed::Separator)),
        '·' | '・' => Some((RULE_MIDDOT, Absorbed::Separator)),
        _ => None,
    }
}

fn char_at(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

/// 숫자 자리로 볼 수 있는 문자인가. 숫자와 마스킹 문자(`*`, `X`, `x`)를 받는다.
///
/// 마스킹 문자를 숫자로 세지 않으면 `1234-****-****-5678` 의 붙임표가 흡수되지 않아
/// 네 조각으로 흩어지고, 카드번호 후보 자체가 만들어지지 않는다.
fn is_digit_like(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '*' | 'X' | 'x')
}

fn ends_with_digit(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_some_and(is_digit_like)
}

/// 구분자 흡수 패스를 적용한다.
pub fn apply(text: &str) -> (String, SpanMap) {
    if !text.chars().any(|c| separator_class(c).is_some()) {
        return (text.to_string(), SpanMap::identity(text));
    }

    let mut builder = SpanMapBuilder::with_capacity(text);
    let mut flushed = 0;
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(c) = char_at(text, cursor) else { break };

        if separator_class(c).is_none() || !ends_with_digit(text, cursor) {
            cursor += c.len_utf8();
            continue;
        }

        // 연속한 구분자를 한 덩어리로 모은다.
        let mut end = cursor;
        while let Some(cj) = char_at(text, end) {
            if separator_class(cj).is_none() {
                break;
            }
            end += cj.len_utf8();
        }

        // 덩어리 뒤가 숫자일 때만 흡수한다.
        if char_at(text, end).is_some_and(is_digit_like) {
            if flushed < cursor {
                builder.keep(&text[flushed..cursor]);
            }
            absorb_run(&mut builder, text, cursor, end);
            flushed = end;
        }

        cursor = end;
    }

    if flushed < text.len() {
        builder.keep(&text[flushed..]);
    }
    builder.finish()
}

/// `[start, end)` 의 구분자들을 같은 규칙끼리 묶어 흡수한다.
///
/// 규칙마다 감점 분류가 다르므로 한 번에 몰아 흡수하지 않고 종류 경계에서 끊는다.
fn absorb_run(builder: &mut SpanMapBuilder, text: &str, start: usize, end: usize) {
    let mut at = start;
    while at < end {
        let Some(c) = char_at(text, at) else { break };
        let Some((rule, class)) = separator_class(c) else { break };

        let mut group_end = at + c.len_utf8();
        while group_end < end {
            let Some(next) = char_at(text, group_end) else { break };
            match separator_class(next) {
                Some((next_rule, _)) if next_rule == rule => group_end += next.len_utf8(),
                _ => break,
            }
        }

        builder.absorb(&text[at..group_end], rule, class);
        at = group_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> String {
        apply(text).0
    }

    #[test]
    fn absorbs_hyphen_between_digits() {
        assert_eq!(run("880101-1234567"), "8801011234567");
    }

    #[test]
    fn absorbs_mixed_separator_run() {
        assert_eq!(run("880101 - 1234567"), "8801011234567");
    }

    #[test]
    fn absorbs_phone_dots() {
        assert_eq!(run("010.1234.5678"), "01012345678");
    }

    #[test]
    fn does_not_join_across_words() {
        // 이 패스가 존재하는 이유이자 가장 중요한 음성 사례다.
        assert_eq!(run("제품 1234 수량 5678"), "제품 1234 수량 5678");
    }

    #[test]
    fn does_not_absorb_leading_or_trailing_separator() {
        assert_eq!(run("- 880101"), "- 880101");
        assert_eq!(run("880101 -"), "880101 -");
    }

    #[test]
    fn does_not_absorb_newline() {
        assert_eq!(run("880101\n1234567"), "880101\n1234567");
    }

    #[test]
    fn counts_cost_by_class() {
        let (out, map) = apply("880101 - 1234567");
        assert_eq!(out, "8801011234567");
        map.validate().unwrap();
        let recovered = map.to_source(&crate::span::Span::new(0..13, 0..13));
        assert_eq!(recovered.cost.absorbed_whitespace, 2, "공백 두 개");
        assert_eq!(recovered.cost.absorbed_separators, 1, "붙임표 한 개");
        assert_eq!(recovered.span.byte, 0..16);
    }
}
