//! 3층: 마스킹.
//!
//! 탐지가 낸 [`Finding`] 의 **원문 스팬**에 정책을 적용한다. 탐지는 정규화문 위에서 하고
//! 마스킹은 원문 위에서 하는데, 둘을 잇는 것이 [`crate::span::SpanMap`] 하나뿐이라
//! 복원 정확성의 책임이 한 곳에 모인다.
//!
//! ## 두 가지 보장
//!
//! | 보장 | 범위 |
//! | --- | --- |
//! | 탐지 구간 **바깥**의 바이트는 원문과 완전히 같다 | 모든 정책 |
//! | `unmask(mask(text).text, &map)` 이 원문과 완전히 같다 | [`Policy::Tokenize`] |
//!
//! 두 번째보다 첫 번째가 더 근본적이다. 이것이 성립하면 마스킹이 문서를 조용히 훼손하는 일이
//! 원천적으로 없다. 구현도 그 모양을 그대로 따른다. 출력은 "직전 구간 끝부터 이번 구간 시작까지를
//! 그대로 복사하고, 구간만 치환한다"의 반복이다. 원문을 파싱하거나 재조립하는 경로가 없다.
//!
//! ```
//! use rust_pii_transformer::detect::Config;
//! use rust_pii_transformer::mask::{mask, unmask, Policy, PolicySet};
//!
//! let text = "주민등록번호 880101-1234568 입니다";
//!
//! // 되돌릴 수 있게 가린다.
//! let out = mask(text, &Config::default(), &PolicySet::new(Policy::Tokenize)).unwrap();
//! assert!(!out.text.contains("880101"));
//!
//! let restored = unmask(&out.text, out.restore.as_ref().unwrap()).unwrap();
//! assert_eq!(restored, text);
//! ```

pub mod policy;
pub mod restore;

use serde::Serialize;

use crate::detect::{detect, Config, Finding, Report};
use crate::error::Result;

pub use policy::{Policy, PolicySet, Redaction};
pub use restore::{unmask, RestoreEntry, RestoreMap};

/// 마스킹 결과.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MaskOutput {
    /// 가려진 텍스트.
    pub text: String,
    /// 실제로 적용된 탐지 결과. 원문 위치 오름차순이다.
    pub applied: Vec<Finding>,
    /// 앞선 구간과 겹쳐서 건너뛴 탐지 결과.
    ///
    /// 비어 있는 것이 정상이다. 비어 있지 않다면 탐지 층이 겹치는 구간을 냈다는 뜻이므로
    /// 조용히 삼키지 않고 여기에 남긴다.
    pub skipped: Vec<Finding>,
    /// 토큰화 정책을 쓴 경우의 복원 맵.
    ///
    /// **이 맵 자체가 개인정보다.** 마스킹 텍스트와 같은 곳에 두면 마스킹한 의미가 없다.
    pub restore: Option<RestoreMap>,
}

/// 텍스트를 탐지하고 곧바로 마스킹한다.
///
/// # Errors
///
/// 탐지 층이 실패하면 그 에러를 그대로 올린다.
pub fn mask(text: &str, cfg: &Config, policies: &PolicySet) -> Result<MaskOutput> {
    let report = detect(text, cfg)?;
    Ok(mask_report(text, &report, policies))
}

/// 이미 낸 탐지 결과로 마스킹한다. 같은 텍스트를 두 번 훑지 않으려는 경우에 쓴다.
pub fn mask_report(text: &str, report: &Report, policies: &PolicySet) -> MaskOutput {
    mask_findings(text, &report.findings, policies)
}

/// 주어진 탐지 결과로 마스킹한다.
///
/// `findings` 는 정렬돼 있지 않아도 된다. 내부에서 원문 위치 순으로 정렬한 뒤 적용한다.
pub fn mask_findings(text: &str, findings: &[Finding], policies: &PolicySet) -> MaskOutput {
    let mut ordered: Vec<Finding> = findings.to_vec();
    ordered.sort_by_key(|f| (f.source.byte.start, f.source.byte.end));

    let mut restore = policies.uses_tokenize().then(|| RestoreMap::for_text(text));
    let mut out = String::with_capacity(text.len());
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    let mut cursor: usize = 0;

    for finding in ordered {
        let start = finding.source.byte.start as usize;
        let end = finding.source.byte.end as usize;

        // 겹치거나 범위를 벗어난 구간은 적용하지 않는다. 조용히 버리지 않고 남긴다.
        if start < cursor || end > text.len() || start > end {
            skipped.push(finding);
            continue;
        }

        // 구간 사이는 원문 그대로 복사한다. 이 한 줄이 "바깥은 손대지 않는다" 보장의 전부다.
        out.push_str(&text[cursor..start]);

        let original = &text[start..end];
        let policy = policies.policy_for(finding.entity);
        out.push_str(&render(policy, original, &finding, restore.as_mut()));

        cursor = end;
        applied.push(finding);
    }

    out.push_str(&text[cursor..]);

    MaskOutput { text: out, applied, skipped, restore }
}

/// 정책 하나를 원문 조각에 적용한다.
fn render(
    policy: &Policy,
    original: &str,
    finding: &Finding,
    restore: Option<&mut RestoreMap>,
) -> String {
    match policy {
        Policy::Redact(Redaction::Label) => format!("[{}]", finding.entity.label()),
        Policy::Redact(Redaction::Code) => format!("[{}]", finding.entity.code_upper()),
        Policy::Redact(Redaction::Fill(ch)) => {
            std::iter::repeat(*ch).take(original.chars().count()).collect()
        }
        Policy::Redact(Redaction::Fixed(s)) => s.clone(),
        Policy::Partial { keep_prefix, keep_suffix, fill } => {
            partial(original, *keep_prefix, *keep_suffix, *fill)
        }
        // 해시 토큰의 접두어는 항상 영문 식별자다. 사람이 읽는 자리표시자가 아니라 기계가
        // 짝을 맞추는 값이고, `code()` 가 JSON·Python·명령줄이 공유하는 그 하나의 이름이다.
        #[cfg(feature = "hash")]
        Policy::Hash { key, len } => {
            format!("[{}:{}]", finding.entity.code_upper(), hmac_hex(key, original, *len))
        }
        Policy::Tokenize => match restore {
            Some(map) => map.push(finding.entity, original),
            // uses_tokenize 가 참일 때만 맵이 만들어지므로 여기 오지 않는다.
            // 그래도 원문을 그대로 흘리는 것보다 가려진 쪽으로 실패한다.
            None => format!("[{}]", finding.entity.label()),
        },
    }
}

/// 앞뒤 일부만 남기고 가운데를 덮는다. 문자 단위로 센다.
fn partial(original: &str, keep_prefix: usize, keep_suffix: usize, fill: char) -> String {
    let chars: Vec<char> = original.chars().collect();
    let total = chars.len();

    // 남길 양이 전체 이상이면 아무것도 남기지 않는다. 짧은 값이 통째로 노출되는 사고를 막는다.
    if keep_prefix + keep_suffix >= total {
        return std::iter::repeat(fill).take(total).collect();
    }

    let mut out = String::with_capacity(original.len());
    out.extend(chars[..keep_prefix].iter());
    out.extend(std::iter::repeat(fill).take(total - keep_prefix - keep_suffix));
    out.extend(chars[total - keep_suffix..].iter());
    out
}

/// 해시 기반 메시지 인증 코드를 16진수 앞자리로 자른다.
#[cfg(feature = "hash")]
fn hmac_hex(key: &[u8], data: &str, len: usize) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC 은 어떤 길이의 열쇠도 받는다");
    mac.update(data.as_bytes());
    let bytes = mac.finalize().into_bytes();

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(len);
    for byte in bytes.iter() {
        if out.len() >= len {
            break;
        }
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        if out.len() >= len {
            break;
        }
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::EntityKind;

    fn run(text: &str, policies: &PolicySet) -> MaskOutput {
        mask(text, &Config::default(), policies).unwrap()
    }

    #[test]
    fn label_redaction_names_what_was_removed() {
        let out = run("주민등록번호 880101-1234568 입니다", &PolicySet::default());
        assert_eq!(out.text, "주민등록번호 [주민등록번호] 입니다");
        assert_eq!(out.applied.len(), 1);
        assert!(out.skipped.is_empty());
        assert!(out.restore.is_none(), "토큰화가 아니면 복원 맵을 만들지 않는다");
    }

    #[test]
    fn fill_redaction_preserves_character_count() {
        let policies = PolicySet::new(Policy::Redact(Redaction::Fill('*')));
        let out = run("카드 4111-1111-1111-1111 결제", &policies);
        assert_eq!(out.text, "카드 ******************* 결제");
        assert_eq!(out.text.matches('*').count(), "4111-1111-1111-1111".chars().count());
    }

    /// 한국어 문서를 다루는 비한국어 사용자를 위한 경로. 산출물에 한국어가 섞이지 않아야 한다.
    #[test]
    fn code_redaction_keeps_the_output_ascii() {
        let policies = PolicySet::new(Policy::Redact(Redaction::Code));
        let out = run("카드 4111-1111-1111-1111 연락처 010-1234-5678", &policies);
        assert_eq!(out.text, "카드 [CREDIT_CARD] 연락처 [PHONE]");

        // 자리표시자 자체에는 한글이 없어야 한다. 원문의 한글은 그대로 남는다.
        for placeholder in ["[CREDIT_CARD]", "[PHONE]"] {
            assert!(placeholder.is_ascii(), "{placeholder} 에 비아스키 문자가 있다");
        }
    }

    #[test]
    fn label_and_code_name_the_same_entity() {
        let text = "주민등록번호 880101-1234568";
        let korean = run(text, &PolicySet::new(Policy::Redact(Redaction::Label)));
        let english = run(text, &PolicySet::new(Policy::Redact(Redaction::Code)));
        assert_eq!(korean.text, "주민등록번호 [주민등록번호]");
        assert_eq!(english.text, "주민등록번호 [RESIDENT]");
        assert_eq!(korean.applied[0].entity, english.applied[0].entity);
    }

    #[test]
    fn fixed_redaction_leaves_no_shape() {
        let policies = PolicySet::new(Policy::Redact(Redaction::Fixed("<가림>".into())));
        let out = run("연락처 010-1234-5678", &policies);
        assert_eq!(out.text, "연락처 <가림>");
    }

    #[test]
    fn partial_keeps_the_ends() {
        let policies = PolicySet::new(Policy::Partial { keep_prefix: 3, keep_suffix: 4, fill: '*' });
        let out = run("연락처 010-1234-5678", &policies);
        assert_eq!(out.text, "연락처 010******5678");
    }

    #[test]
    fn partial_hides_everything_when_the_value_is_too_short() {
        // 남길 양이 전체보다 크면 통째로 덮는다. 짧은 값이 그대로 노출되면 안 된다.
        assert_eq!(partial("0101234", 5, 5, '*'), "*******");
        assert_eq!(partial("880101-1234568", 3, 4, '#'), "880#######4568");
    }

    #[test]
    fn partial_counts_characters_not_bytes() {
        // 한글 한 글자는 3바이트다. 글자로 세지 않으면 문자 경계가 깨진다.
        assert_eq!(partial("가나다라마", 1, 1, '*'), "가***마");
    }

    #[test]
    fn per_entity_policies_are_applied() {
        let policies = PolicySet::new(Policy::Redact(Redaction::Label))
            .with(EntityKind::Phone, Policy::Partial { keep_prefix: 3, keep_suffix: 4, fill: '*' });
        let out = run("카드 4111-1111-1111-1111 연락처 010-1234-5678", &policies);
        assert_eq!(out.text, "카드 [카드번호] 연락처 010******5678");
    }

    // ── 복원 보장 ──────────────────────────────────────────

    #[test]
    fn tokenize_round_trips_exactly() {
        let text = "주민등록번호 880101-1234568 이고 카드 4111-1111-1111-1111 이며 메일 a@b.com";
        let out = run(text, &PolicySet::new(Policy::Tokenize));
        assert!(!out.text.contains("880101"));
        assert!(!out.text.contains("4111"));
        assert!(!out.text.contains("a@b.com"));

        let map = out.restore.as_ref().unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(unmask(&out.text, map).unwrap(), text);
    }

    #[test]
    fn tokenize_round_trips_through_hangul_numerals() {
        // 원문이 한글 수사인 경우가 복원의 최악 조건이다. 정규화가 글자 수를 바꿔 놓기 때문이다.
        let text = "주민등록번호 팔팔공일공일 - 1234567 입니다";
        let out = run(text, &PolicySet::new(Policy::Tokenize));
        assert!(!out.text.contains("팔팔공일공일"));
        assert_eq!(unmask(&out.text, out.restore.as_ref().unwrap()).unwrap(), text);
    }

    #[test]
    fn tokens_do_not_collide_with_lookalike_text() {
        let text = "이전 로그 [[PII:0]] 옆에 주민등록번호 880101-1234568";
        let out = run(text, &PolicySet::new(Policy::Tokenize));
        let map = out.restore.as_ref().unwrap();
        assert_ne!(map.prefix(), "PII");
        assert_eq!(unmask(&out.text, map).unwrap(), text, "원문의 가짜 토큰은 그대로 남아야 한다");
    }

    #[test]
    fn bytes_outside_findings_are_untouched_for_every_policy() {
        let text = "앞 문장. 연락처 010-1234-5678 뒤 문장.";
        let policies = [
            PolicySet::new(Policy::Redact(Redaction::Label)),
            PolicySet::new(Policy::Redact(Redaction::Fill('*'))),
            PolicySet::new(Policy::Partial { keep_prefix: 3, keep_suffix: 4, fill: '*' }),
            PolicySet::new(Policy::Tokenize),
        ];
        for policies in policies {
            let out = run(text, &policies);
            assert!(out.text.starts_with("앞 문장. 연락처 "), "{}", out.text);
            assert!(out.text.ends_with(" 뒤 문장."), "{}", out.text);
        }
    }

    #[test]
    fn a_text_without_findings_is_returned_verbatim() {
        let text = "이번 분기 실적은 전년 대비 개선되었습니다.";
        let out = run(text, &PolicySet::new(Policy::Tokenize));
        assert_eq!(out.text, text);
        assert!(out.applied.is_empty());
        assert!(out.restore.as_ref().unwrap().is_empty());
        assert_eq!(unmask(&out.text, out.restore.as_ref().unwrap()).unwrap(), text);
    }

    #[test]
    fn overlapping_findings_are_skipped_not_swallowed() {
        let text = "카드 4111-1111-1111-1111 입니다";
        let report = detect(text, &Config::default()).unwrap();
        let mut findings = report.findings.clone();
        // 같은 구간을 한 번 더 밀어 넣어 겹침을 강제한다.
        findings.push(report.findings[0].clone());

        let out = mask_findings(text, &findings, &PolicySet::default());
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.skipped.len(), 1, "겹친 것은 버리지 않고 남긴다");
        assert_eq!(out.text, "카드 [카드번호] 입니다");
    }

    #[test]
    fn findings_do_not_need_to_be_sorted() {
        let text = "카드 4111-1111-1111-1111 연락처 010-1234-5678";
        let report = detect(text, &Config::default()).unwrap();
        let mut reversed = report.findings.clone();
        reversed.reverse();

        let a = mask_findings(text, &report.findings, &PolicySet::default());
        let b = mask_findings(text, &reversed, &PolicySet::default());
        assert_eq!(a.text, b.text);
        assert!(b.skipped.is_empty());
    }

    #[cfg(feature = "hash")]
    #[test]
    fn hash_is_deterministic_and_links_equal_values() {
        let policies = PolicySet::new(Policy::Hash { key: b"secret".to_vec(), len: 8 });
        let text = "연락처 010-1234-5678 과 010-1234-5678 은 같은 번호";
        let out = run(text, &policies);

        let first = out.text.find("[PHONE:").unwrap();
        let second = out.text.rfind("[PHONE:").unwrap();
        assert_ne!(first, second, "두 번 나와야 한다");
        assert_eq!(
            &out.text[first..first + 16],
            &out.text[second..second + 16],
            "같은 값은 같은 토큰이 되어야 연결성 분석이 된다"
        );
    }

    #[cfg(feature = "hash")]
    #[test]
    fn hash_changes_with_the_key() {
        let text = "연락처 010-1234-5678";
        let a = run(text, &PolicySet::new(Policy::Hash { key: b"one".to_vec(), len: 12 }));
        let b = run(text, &PolicySet::new(Policy::Hash { key: b"two".to_vec(), len: 12 }));
        assert_ne!(a.text, b.text, "열쇠가 다르면 가명도 달라야 한다");
    }
}
