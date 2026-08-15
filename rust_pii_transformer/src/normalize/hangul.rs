//! 3번 패스: 한글 수사 역변환.
//!
//! `팔팔공일공일` 을 `880101` 로, `구십일` 을 `91` 로 옮긴다. 순수 숫자열만 전제하는
//! 기존 탐지기가 통째로 놓치는 표기이고, 이 라이브러리가 존재하는 이유다.
//!
//! ## 두 가지 읽기 문법
//!
//! 한국어 수사 표기는 문법이 둘이라 표 하나로 끝나지 않는다.
//!
//! - **자릿수 읽기**. 음절 하나가 숫자 한 자리다. `팔팔공일공일` → `880101`, `공일공` → `010`
//! - **단위 읽기**. 십·백·천·만·억을 계산한다. `구십일` → `91`, `천구백팔십팔` → `1988`
//!
//! 판별은 단순하다. **런 안에 단위 음절이 하나라도 있으면 단위 읽기, 없으면 자릿수 읽기.**
//!
//! ## 오탐 억제
//!
//! `사구`, `이사`, `구이` 처럼 수사 음절과 일상어가 겹치는 경우가 많다. 그래서 세 겹으로 막는다.
//!
//! 1. **자릿수 문턱**. 결과 자릿수가 [`NumeralConfig::min_digits_without_context`] 이상이면
//!    문맥 없이 변환한다. 기본 6이라 주민등록번호 앞자리 길이가 그대로 기준이 된다
//! 2. **숫자 문맥 인접**. 그보다 짧으면 `년` `월` `일` 같은 숫자 표지나 인접 숫자·붙임표가
//!    있을 때만 변환한다
//! 3. **후단 검증에 위임**. 위 둘을 통과해도 최종 판정은 탐지 층의 체크섬과 날짜 유효성이 한다
//!
//! 단위 읽기에는 조건이 하나 더 붙는다. **숫자 음절이 하나도 없는 런은 변환하지 않는다.**
//! `만` `천` `백` 같은 단위 음절 홀로는 일상어일 확률이 압도적으로 높기 때문이다.

use crate::span::{RuleId, SpanMap, SpanMapBuilder};

/// 자릿수 읽기로 변환했음을 뜻한다.
pub const RULE_DIGIT: RuleId = "hangul.digit_reading";
/// 단위 읽기로 변환했음을 뜻한다.
pub const RULE_UNIT: RuleId = "hangul.unit_reading";

/// 한글 수사 역변환의 오탐 억제 설정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumeralConfig {
    /// 문맥 단서 없이 변환할 최소 결과 자릿수. 기본 6.
    pub min_digits_without_context: usize,
    /// 문맥 단서가 있을 때 변환할 최소 결과 자릿수. 기본 2.
    pub min_digits_with_context: usize,
    /// 문맥 단서를 찾을 때 건너뛸 공백의 최대 개수. 기본 2.
    pub context_window: usize,
}

impl Default for NumeralConfig {
    fn default() -> Self {
        Self {
            min_digits_without_context: 6,
            min_digits_with_context: 2,
            context_window: 2,
        }
    }
}

/// 두 음절짜리 고유어 수사. 한 음절 표보다 **먼저** 봐야 `일곱` 이 `일` 로 잘리지 않는다.
const DIGIT_TWO_SYLLABLE: &[(&str, u8)] = &[
    ("하나", 1),
    ("다섯", 5),
    ("여섯", 6),
    ("일곱", 7),
    ("여덟", 8),
    ("아홉", 9),
];

/// 한 음절 수사. 한자어 계열과 고유어 계열을 함께 담는다.
const DIGIT_ONE_SYLLABLE: &[(char, u8)] = &[
    ('공', 0),
    ('영', 0),
    ('빵', 0),
    ('일', 1),
    ('이', 2),
    ('삼', 3),
    ('사', 4),
    ('오', 5),
    ('육', 6),
    ('륙', 6),
    ('칠', 7),
    ('팔', 8),
    ('구', 9),
    ('둘', 2),
    ('셋', 3),
    ('넷', 4),
];

/// 단위 음절과 그 크기.
const UNIT_SYLLABLE: &[(char, u64)] = &[
    ('십', 10),
    ('백', 100),
    ('천', 1_000),
    ('만', 10_000),
    ('억', 100_000_000),
];

/// 숫자 표지. 런 앞뒤에 이것이 붙어 있으면 숫자 문맥으로 본다.
const NUMERIC_MARKERS: &[char] = &[
    '년', '월', '일', '번', '호', '생', '세', '원', '차', '기', '시', '분', '초',
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Digit(u8),
    Unit(u64),
}

/// `rest` 의 맨 앞에서 수사 토큰 하나를 읽는다. 읽은 토큰과 소비한 바이트 수를 낸다.
fn next_token(rest: &str) -> Option<(Token, usize)> {
    for &(word, value) in DIGIT_TWO_SYLLABLE {
        if rest.starts_with(word) {
            return Some((Token::Digit(value), word.len()));
        }
    }
    let c = rest.chars().next()?;
    for &(syllable, value) in DIGIT_ONE_SYLLABLE {
        if syllable == c {
            return Some((Token::Digit(value), c.len_utf8()));
        }
    }
    for &(syllable, value) in UNIT_SYLLABLE {
        if syllable == c {
            return Some((Token::Unit(value), c.len_utf8()));
        }
    }
    None
}

/// 이 문자로 시작하는 수사 토큰이 있을 수 있는가. 빠른 경로 판정에만 쓴다.
fn may_start_numeral(c: char) -> bool {
    DIGIT_ONE_SYLLABLE.iter().any(|&(s, _)| s == c)
        || UNIT_SYLLABLE.iter().any(|&(s, _)| s == c)
        || DIGIT_TWO_SYLLABLE.iter().any(|&(w, _)| w.starts_with(c))
}

/// 단위 읽기를 계산한다. 자리올림이 `u64` 를 넘치면 변환을 포기한다.
///
/// `구십일` = 구(9) 십(9 x 10 = 90) 일(1) → 91 처럼, 단위를 만날 때마다 앞의 숫자를 곱해
/// 구간 합에 넣고, 만·억을 만나면 구간을 통째로 곱해 총합으로 옮긴다.
fn eval_unit_reading(tokens: &[Token]) -> Option<u64> {
    let mut total: u64 = 0;
    let mut section: u64 = 0;
    let mut current: u64 = 0;

    for token in tokens {
        match *token {
            Token::Digit(d) => {
                // 단위 사이에 숫자가 둘 이상 오는 표기(`이삼십`)는 정상 수사가 아니다.
                if current != 0 {
                    return None;
                }
                current = u64::from(d);
            }
            Token::Unit(u) if u >= 10_000 => {
                let multiplicand = section.checked_add(current)?;
                let multiplicand = if multiplicand == 0 { 1 } else { multiplicand };
                total = total.checked_add(multiplicand.checked_mul(u)?)?;
                section = 0;
                current = 0;
            }
            Token::Unit(u) => {
                let multiplicand = if current == 0 { 1 } else { current };
                section = section.checked_add(multiplicand.checked_mul(u)?)?;
                current = 0;
            }
        }
    }

    total.checked_add(section)?.checked_add(current)
}

/// 토큰 런을 숫자 문자열로 바꾼다. 바꿀 수 없으면 `None`.
fn convert(tokens: &[Token]) -> Option<(String, RuleId)> {
    let has_unit = tokens.iter().any(|t| matches!(t, Token::Unit(_)));
    let has_digit = tokens.iter().any(|t| matches!(t, Token::Digit(_)));

    if has_unit {
        // 단위 음절만으로 된 런(`만`, `천만`)은 일상어일 확률이 압도적이다.
        if !has_digit {
            return None;
        }
        let value = eval_unit_reading(tokens)?;
        Some((value.to_string(), RULE_UNIT))
    } else {
        // 자릿수 읽기는 앞자리 0 을 살려야 하므로 수치가 아니라 문자열로 잇는다.
        let mut out = String::with_capacity(tokens.len());
        for token in tokens {
            match token {
                Token::Digit(d) => out.push(char::from(b'0' + d)),
                Token::Unit(_) => return None,
            }
        }
        Some((out, RULE_DIGIT))
    }
}

/// 런 바로 앞에서, 공백을 최대 `window` 개까지 건너뛴 첫 글자.
fn char_before(text: &str, at: usize, window: usize) -> Option<char> {
    let mut skipped = 0;
    for c in text[..at].chars().rev() {
        if c.is_whitespace() {
            skipped += 1;
            if skipped > window {
                return None;
            }
            continue;
        }
        return Some(c);
    }
    None
}

/// 런 바로 뒤에서, 공백을 최대 `window` 개까지 건너뛴 첫 글자.
fn char_after(text: &str, at: usize, window: usize) -> Option<char> {
    let mut skipped = 0;
    for c in text[at..].chars() {
        if c.is_whitespace() {
            skipped += 1;
            if skipped > window {
                return None;
            }
            continue;
        }
        return Some(c);
    }
    None
}

fn is_numeric_context(c: char) -> bool {
    c.is_ascii_digit() || c == '-' || NUMERIC_MARKERS.contains(&c)
}

/// 런 `[start, end)` 주변에 숫자 문맥이 있는가.
fn has_numeric_context(text: &str, start: usize, end: usize, cfg: &NumeralConfig) -> bool {
    char_before(text, start, cfg.context_window).is_some_and(is_numeric_context)
        || char_after(text, end, cfg.context_window).is_some_and(is_numeric_context)
}

/// 한글 수사 역변환 패스를 적용한다.
/// 수사 런 끝에 붙어 삼켜지는 말끝. `(런의 끝, 그 뒤에 오는 글자)` 짝이다.
///
/// 문제는 어미가 **수사 음절로 시작한다**는 것이다. `구공오삼일삼이래요` 에서 `이` 는 숫자 2 이고
/// `구` 는 9 라, 런을 욕심껏 모으면 `이` 까지 먹어 `9053132` 가 된다. 한 자리가 더 붙으면
/// 주민등록번호 앞자리로 안 읽힌다. 음성 전사(STT) 입력에서 실제로 나는 일이다.
///
/// 긴 것부터 본다. `이구요` 는 `이구` 를 떼야 하고 `이요` 는 `이` 를 뗀다.
const SENTENCE_ENDINGS: &[(&str, &str)] = &[
    ("이구", "요"),
    ("이", "래요"),
    ("이", "에요"),
    ("이", "예요"),
    ("이", "요"),
    ("이", "고"),
];

/// 런 끝에 말끝이 삼켜져 있으면 런을 그만큼 줄인다.
///
/// 떼어 낸 음절은 버리지 않는다. 다음 순회에서 다시 훑히고, 한 음절짜리 런은 최소 자릿수에
/// 못 미쳐 변환되지 않으므로 원문 그대로 남는다.
fn trim_sentence_ending(text: &str, start: usize, end: usize) -> usize {
    let run = &text[start..end];
    let rest = &text[end..];
    for (tail, follows) in SENTENCE_ENDINGS {
        if run.ends_with(tail) && rest.starts_with(follows) && run.len() > tail.len() {
            return end - tail.len();
        }
    }
    end
}

pub fn apply(text: &str, cfg: &NumeralConfig) -> (String, SpanMap) {
    if !text.chars().any(may_start_numeral) {
        return (text.to_string(), SpanMap::identity(text));
    }

    let mut builder = SpanMapBuilder::with_capacity(text);
    let mut flushed = 0;
    let mut cursor = 0;

    while cursor < text.len() {
        // 이 위치에서 시작하는 수사 런을 최대한 길게 모은다.
        let mut end = cursor;
        while let Some((_, len)) = next_token(&text[end..]) {
            end += len;
        }

        // 말끝이 삼켜졌으면 런을 줄이고 그 범위로 다시 토큰을 모은다.
        end = trim_sentence_ending(text, cursor, end);

        let mut tokens = Vec::new();
        let mut walk = cursor;
        while walk < end {
            let Some((token, len)) = next_token(&text[walk..]) else { break };
            if walk + len > end {
                break;
            }
            tokens.push(token);
            walk += len;
        }

        if tokens.is_empty() {
            cursor += text[cursor..].chars().next().map_or(1, char::len_utf8);
            continue;
        }

        if let Some((digits, rule)) = convert(&tokens) {
            let long_enough = digits.len() >= cfg.min_digits_without_context
                || (digits.len() >= cfg.min_digits_with_context
                    && has_numeric_context(text, cursor, end, cfg));
            if long_enough {
                if flushed < cursor {
                    builder.keep(&text[flushed..cursor]);
                }
                builder.numeral(&text[cursor..end], &digits, rule);
                flushed = end;
            }
        }

        cursor = end;
    }

    if flushed < text.len() {
        builder.keep(&text[flushed..]);
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> String {
        apply(text, &NumeralConfig::default()).0
    }

    /// 음성 전사 입력. 어미가 수사 음절로 시작해 런에 삼켜지던 자리다.
    ///
    /// 떼어 낸 음절은 사라지지 않고 원문 그대로 남는다 — 숫자로 읽지 않을 뿐이다.
    #[test]
    fn sentence_endings_are_not_swallowed_into_the_run() {
        assert_eq!(run("구공오삼일삼이래요"), "905313이래요");
        assert_eq!(run("구공오삼일삼이에요"), "905313이에요");
        assert_eq!(run("구공오삼일삼이예요"), "905313이예요");
        assert_eq!(run("구공오삼일삼이요"), "905313이요");
        assert_eq!(run("구공오삼일삼이고"), "905313이고");
        assert_eq!(run("구공오삼일삼이구요"), "905313이구요");
    }

    #[test]
    fn a_trailing_numeral_that_is_not_an_ending_stays_in_the_run() {
        // 뒤에 말끝이 없으면 `이` 는 그냥 숫자 2 다.
        assert_eq!(run("구공오삼일삼이"), "9053132");
    }

    #[test]
    fn digit_reading_converts_long_run() {
        assert_eq!(run("팔팔공일공일"), "880101");
    }

    #[test]
    fn digit_reading_preserves_leading_zero() {
        assert_eq!(run("공일공일이삼사"), "0101234");
    }

    #[test]
    fn digit_reading_needs_context_when_short() {
        // 세 자리는 문맥이 없으면 그대로 둔다.
        assert_eq!(run("공일공"), "공일공");
        // 뒤에 숫자 표지가 붙으면 변환한다.
        assert_eq!(run("공일공번"), "010번");
    }

    #[test]
    fn unit_reading_computes_value() {
        assert_eq!(run("구십일년"), "91년");
        assert_eq!(run("이천이십사년"), "2024년");
        assert_eq!(run("천구백팔십팔년"), "1988년");
    }

    #[test]
    fn unit_reading_handles_man_and_eok() {
        assert_eq!(run("일억이천만원"), "120000000원");
    }

    #[test]
    fn unit_only_run_is_left_alone() {
        // 숫자 음절이 없는 단위어는 일상어로 본다.
        assert_eq!(run("만원"), "만원");
        assert_eq!(run("천만"), "천만");
    }

    #[test]
    fn everyday_words_are_left_alone() {
        assert_eq!(run("사구려"), "사구려");
        assert_eq!(run("이사 갑니다"), "이사 갑니다");
        assert_eq!(run("구이를 먹었다"), "구이를 먹었다");
    }

    #[test]
    fn native_numerals_are_read() {
        assert_eq!(run("하나둘셋넷다섯여섯"), "123456");
    }

    #[test]
    fn seven_is_not_split_into_one() {
        // `일곱` 을 `일` + `곱` 으로 자르면 런이 끊긴다. 두 음절 표를 먼저 보는 이유다.
        assert_eq!(run("일곱여덟아홉하나둘셋"), "789123");
    }

    #[test]
    fn rejects_malformed_unit_sequence() {
        // `이삼십` 은 정상 수사가 아니다. 계산을 포기하고 원문을 그대로 둔다.
        assert_eq!(run("이삼십년"), "이삼십년");
    }

    #[test]
    fn map_recovers_source_span() {
        let src = "팔팔공일공일";
        let (out, map) = apply(src, &NumeralConfig::default());
        assert_eq!(out, "880101");
        map.validate().unwrap();
        let recovered = map.to_source(&crate::span::Span::new(0..6, 0..6));
        assert_eq!(recovered.span.byte, 0..18, "한글 6음절은 18바이트다");
        assert_eq!(recovered.cost.expanded_syllables, 6);
        assert_eq!(recovered.rules, vec![RULE_DIGIT]);
    }

    #[test]
    fn threshold_is_configurable() {
        let cfg = NumeralConfig {
            min_digits_without_context: 3,
            ..Default::default()
        };
        assert_eq!(apply("공일공", &cfg).0, "010");
    }
}
