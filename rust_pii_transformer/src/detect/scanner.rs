//! 정규화문에서 후보 구간을 뽑아내는 스캐너.
//!
//! 정규화 층이 이미 구분자를 흡수했으므로 숫자 계열 엔티티는 **숫자 런을 모으고 길이로
//! 분류하는 것**으로 환원된다. 정규 표현식보다 빠르고 결정적이며, 파국적 역추적이 원리적으로
//! 없다.
//!
//! ## 이메일도 손으로 스캔한다
//!
//! 설계 문서는 이메일처럼 구조가 복잡한 항목에 정규 표현식을 쓴다고 적었지만, 구현은
//! **정규 표현식 크레이트를 들이지 않고 직접 스캔한다.** `regex` 는 순수 Rust 라 외부 함수
//! 인터페이스 0 원칙을 깨지는 않지만 전이 의존성을 넷 이상 끌고 온다. 이메일 스캔은 골뱅이를
//! 찾아 양옆으로 넓히는 40줄짜리 일이라 그 비용을 낼 이유가 없다. 의존성은 `thiserror` 와
//! `serde` 둘로 유지된다.

use crate::span::Span;

/// 골뱅이 왼쪽에 올 수 있는 문자인가.
fn is_local_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

/// 골뱅이 오른쪽에 올 수 있는 문자인가.
fn is_domain_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// 숫자 런에 낄 수 있는 문자인가. 숫자와 마스킹 문자(`*`, `X`, `x`)를 받는다.
///
/// 마스킹 문자를 런에 넣는 이유는 **이미 부분 마스킹된 문서를 다시 처리**해야 하기 때문이다.
/// `900513-1****67` 을 숫자가 아니라는 이유로 버리면 개인정보가 그대로 통과한다.
fn is_run_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '*' | 'X' | 'x')
}

/// 런이 후보가 될 자격이 있는가.
///
/// 실제 숫자가 **두 자리 이상** 있어야 한다. 이 문턱이 없으면 마크다운 강조(`**`)나
/// 곱셈 표기(`3x4`), 낱글자 `X` 가 전부 후보가 되어 잡음이 폭증한다.
fn run_is_candidate(text: &str) -> bool {
    text.chars().filter(|c| c.is_ascii_digit()).count() >= 2
}

/// 연속한 숫자 구간을 모두 찾는다. 마스킹 문자가 섞인 구간도 포함한다.
///
/// 반환 스팬은 **정규화문 좌표계**다. 원문 좌표로 되돌리는 것은 호출자의 일이다.
pub fn digit_runs(text: &str) -> Vec<Span> {
    let mut runs = Vec::new();
    let mut start: Option<(usize, u32)> = None;
    let mut char_index = 0u32;

    let close = |runs: &mut Vec<Span>,
                 byte_start: usize,
                 char_start: u32,
                 byte_end: usize,
                 char_end: u32| {
        if run_is_candidate(&text[byte_start..byte_end]) {
            runs.push(Span::new(
                byte_start as u32..byte_end as u32,
                char_start..char_end,
            ));
        }
    };

    for (byte_index, c) in text.char_indices() {
        if is_run_char(c) {
            if start.is_none() {
                start = Some((byte_index, char_index));
            }
        } else if let Some((byte_start, char_start)) = start.take() {
            close(&mut runs, byte_start, char_start, byte_index, char_index);
        }
        char_index += 1;
    }

    if let Some((byte_start, char_start)) = start {
        close(&mut runs, byte_start, char_start, text.len(), char_index);
    }
    runs
}

/// 도메인 부분이 그럴듯한가.
///
/// 점이 하나 이상 있고, 마지막 조각이 두 글자 이상의 알파벳이며, 빈 조각이 없어야 한다.
fn plausible_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// 이메일 주소로 보이는 구간을 모두 찾는다.
pub fn emails(text: &str) -> Vec<Span> {
    if !text.contains('@') {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        if chars[at].1 != '@' {
            at += 1;
            continue;
        }

        // 골뱅이 왼쪽으로 넓힌다.
        let mut start = at;
        while start > 0 && is_local_part(chars[start - 1].1) {
            start -= 1;
        }
        // 점으로 시작하거나 끝나는 로컬 파트는 주소가 아니다.
        while start < at && chars[start].1 == '.' {
            start += 1;
        }

        // 골뱅이 오른쪽으로 넓힌다.
        let mut end = at + 1;
        while end < chars.len() && is_domain_part(chars[end].1) {
            end += 1;
        }
        // 문장 끝의 마침표나 붙임표는 주소의 일부가 아니다.
        while end > at + 1 && matches!(chars[end - 1].1, '.' | '-') {
            end -= 1;
        }

        let local_ok = start < at && chars[at - 1].1 != '.';
        if local_ok && end > at + 1 {
            let domain: String = chars[at + 1..end].iter().map(|&(_, c)| c).collect();
            if plausible_domain(&domain) {
                let byte_start = chars[start].0;
                let byte_end = chars.get(end).map_or(text.len(), |&(byte, _)| byte);
                found.push(Span::new(
                    byte_start as u32..byte_end as u32,
                    start as u32..end as u32,
                ));
                at = end;
                continue;
            }
        }
        at += 1;
    }
    found
}

/// 여권번호로 볼 수 있는 구간을 모두 찾는다.
///
/// 모양은 **영문 대문자 1-2 자 + 숫자 7-8 자**, 전체 8-9 자다. 대한민국 여권은 `M12345678`
/// (구권 `S1234567`) 처럼 한 글자로 시작하고, 다른 나라 여권은 두 글자로 시작하는 것이 흔하다.
///
/// ## 이 모양은 검증식이 없다
///
/// 여권번호에는 공개된 검사 숫자가 없다. 그래서 이 스캐너가 내는 것은 **문맥이 있어야 살아남는
/// 후보**다. 모양만 보면 `A1234567` 같은 제품 코드·주문 번호와 구분되지 않고, 실제로 그런
/// 문자열은 문서에 흔하다. 판정은 [`super::context`] 의 여권 단서 목록이 한다.
///
/// 반환 스팬은 **정규화문 좌표계**다.
pub fn passports(text: &str) -> Vec<Span> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        // 앞이 영숫자면 더 긴 토큰의 일부다. 토큰 경계에서만 시작한다.
        if at > 0 && chars[at - 1].1.is_ascii_alphanumeric() {
            at += 1;
            continue;
        }

        let letters = chars[at..]
            .iter()
            .take(2)
            .take_while(|&&(_, c)| c.is_ascii_uppercase())
            .count();
        if letters == 0 {
            at += 1;
            continue;
        }

        let digit_start = at + letters;
        let digits = chars[digit_start..]
            .iter()
            .take_while(|&&(_, c)| c.is_ascii_digit())
            .count();
        let end = digit_start + digits;

        // 뒤가 영숫자면 잘라 낸 조각이므로 후보가 아니다.
        let bounded = chars
            .get(end)
            .map_or(true, |&(_, c)| !c.is_ascii_alphanumeric());
        let shaped = (7..=8).contains(&digits) && (8..=9).contains(&(letters + digits));

        if shaped && bounded {
            let byte_start = chars[at].0;
            let byte_end = chars.get(end).map_or(text.len(), |&(byte, _)| byte);
            found.push(Span::new(
                byte_start as u32..byte_end as u32,
                at as u32..end as u32,
            ));
        }
        at = if end > at { end } else { at + 1 };
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices<'a>(text: &'a str, spans: &[Span]) -> Vec<&'a str> {
        spans.iter().map(|s| s.slice(text)).collect()
    }

    #[test]
    fn finds_passport_shapes() {
        let text = "여권 M12345678 과 구권 S1234567 그리고 AB1234567";
        assert_eq!(
            slices(text, &passports(text)),
            vec!["M12345678", "S1234567", "AB1234567"]
        );
    }

    #[test]
    fn rejects_shapes_that_are_not_passports() {
        for text in [
            "M123456",     // 숫자 여섯
            "M123456789",  // 숫자 아홉
            "ABC1234567",  // 영문 셋
            "m12345678",   // 소문자
            "1234567890",  // 영문 없음
            "XM12345678",  // 영문 셋과 같은 폭
            "M12345678A",  // 뒤가 이어진다
            "ZM12345678",  // 앞 토큰의 일부
        ] {
            assert!(passports(text).is_empty(), "여권이 아닌데 잡혔다: {text}");
        }
    }

    #[test]
    fn passport_keeps_byte_and_char_offsets_separate() {
        let text = "여권 M12345678";
        let span = &passports(text)[0];
        assert_eq!(span.byte.start, 7);
        assert_eq!(span.char.start, 3);
        assert_eq!(span.slice(text), "M12345678");
    }

    #[test]
    fn a_passport_glued_to_a_word_is_not_a_candidate() {
        // 앞뒤 경계를 안 보면 상품 코드 꼬리가 전부 여권이 된다.
        assert!(passports("SKU-XM12345678").is_empty());
    }

    #[test]
    fn finds_digit_runs() {
        let text = "주민 8801011234567 카드 4111111111111111 끝";
        let runs = digit_runs(text);
        assert_eq!(
            slices(text, &runs),
            vec!["8801011234567", "4111111111111111"]
        );
    }

    #[test]
    fn digit_run_at_the_very_end_is_closed() {
        let text = "번호 01012345678";
        let runs = digit_runs(text);
        assert_eq!(slices(text, &runs), vec!["01012345678"]);
    }

    #[test]
    fn digit_run_keeps_byte_and_char_offsets_separate() {
        // 한글 두 글자는 6바이트, 2문자다.
        let text = "번호 880101";
        let run = &digit_runs(text)[0];
        assert_eq!(run.byte.start, 7, "한글 2자 6바이트 + 공백 1바이트");
        assert_eq!(run.char.start, 3, "한글 2자 + 공백 1자");
        assert_eq!(run.slice(text), "880101");
    }

    #[test]
    fn no_runs_in_plain_text() {
        assert!(digit_runs("숫자가 하나도 없는 문장").is_empty());
    }

    #[test]
    fn finds_emails() {
        let text = "연락은 minsu.kim@example.com 또는 a_b+c@sub.example.co.kr 로";
        let spans = emails(text);
        assert_eq!(
            slices(text, &spans),
            vec!["minsu.kim@example.com", "a_b+c@sub.example.co.kr"]
        );
    }

    #[test]
    fn trims_sentence_punctuation_from_domain() {
        let text = "메일은 kim@example.com. 입니다";
        assert_eq!(slices(text, &emails(text)), vec!["kim@example.com"]);
    }

    #[test]
    fn rejects_shapes_that_are_not_addresses() {
        for text in [
            "@example.com",     // 로컬 파트 없음
            "kim@",             // 도메인 없음
            "kim@example",      // 점 없음
            "kim@example.c",    // 최상위 도메인이 한 글자
            "kim@example.12",   // 최상위 도메인이 숫자
            "kim@.com",         // 빈 라벨
            "가격은 100@200 원", // 골뱅이를 구분자로 쓴 문장
        ] {
            assert!(emails(text).is_empty(), "주소가 아닌데 잡혔다: {text}");
        }
    }

    #[test]
    fn no_at_sign_takes_the_fast_path() {
        assert!(emails("골뱅이가 없는 문장").is_empty());
    }
}
