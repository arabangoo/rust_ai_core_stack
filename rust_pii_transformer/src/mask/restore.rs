//! 토큰화 마스킹의 복원 맵.
//!
//! 이 모듈이 지키는 약속은 하나다. **`unmask(mask(text).text, &map)` 이 원문과 바이트 단위로
//! 완전히 같다.** Presidio 가 문서의 36퍼센트에서 실패하는 지점이 정확히 여기다.
//!
//! 그 약속이 성립하는 근거는 두 가지다. 토큰이 원문에 존재하지 않는 문자열임을 **만들 때 확인**하고,
//! 항목이 원문 조각을 바이트 그대로 들고 있다는 것이다. 추정이 끼어들 자리가 없다.

use serde::{Deserialize, Serialize};

use crate::detect::EntityKind;
use crate::error::{Error, Result};

/// 토큰 하나와 그것이 대신한 원문 조각.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreEntry {
    /// 마스킹 결과에 박힌 토큰. `[[PII:0]]`
    pub token: String,
    /// 그 자리에 있던 원문 조각. 바이트 그대로다.
    pub original: String,
    /// 무슨 엔티티였는가.
    pub entity: EntityKind,
}

/// 토큰과 원문을 잇는 표.
///
/// 마스킹 결과와 함께 보관해야 복원할 수 있다. **이 표 자체가 개인정보다.** 마스킹 텍스트와
/// 같은 곳에 두면 마스킹한 의미가 없어진다.
///
/// 파일로 저장했다가 다시 읽을 수 있도록 `Deserialize` 도 유도한다. 프로세스가 끝나면 복원이
/// 불가능해지면 가역 정책이라고 부를 수 없기 때문이다.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RestoreMap {
    prefix: String,
    entries: Vec<RestoreEntry>,
}

impl RestoreMap {
    /// 원문과 충돌하지 않는 토큰 접두어를 골라 빈 표를 만든다.
    ///
    /// `[[PII:` 가 원문에 이미 있으면 `[[PII0:`, `[[PII1:` 로 번호를 올려 가며 비어 있는 이름을
    /// 찾는다. 결정적이고, 어떤 입력에서도 충돌하지 않는다.
    pub(crate) fn for_text(text: &str) -> Self {
        let mut prefix = String::from("PII");
        let mut nth = 0u32;
        while text.contains(&format!("[[{prefix}:")) {
            prefix = format!("PII{nth}");
            nth += 1;
        }
        Self { prefix, entries: Vec::new() }
    }

    /// 원문 조각을 등록하고 그 자리에 넣을 토큰을 돌려준다.
    pub(crate) fn push(&mut self, entity: EntityKind, original: &str) -> String {
        let token = format!("[[{}:{}]]", self.prefix, self.entries.len());
        self.entries.push(RestoreEntry {
            token: token.clone(),
            original: original.to_string(),
            entity,
        });
        token
    }

    /// 이 표가 쓰는 토큰 접두어.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// 등록된 항목들.
    pub fn entries(&self) -> &[RestoreEntry] {
        &self.entries
    }

    /// 항목이 하나도 없는가.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 항목 수.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// 마스킹된 텍스트를 원문으로 되돌린다.
///
/// 토큰화 정책으로 가린 구간만 되돌아온다. 전체 치환이나 부분 노출로 가린 구간은 원문이
/// 남아 있지 않으므로 되돌아오지 않는다. 불가역 정책의 정의가 그렇다.
///
/// # Errors
///
/// 토큰 모양이 깨졌거나 표에 없는 번호를 가리키면 [`Error::RestoreToken`].
/// 조용히 넘기지 않는 이유는, 복원 실패를 눈치채지 못한 채 "원문을 되찾았다"고 믿는 것이
/// 이 라이브러리가 막으려는 바로 그 사고이기 때문이다.
pub fn unmask(masked: &str, map: &RestoreMap) -> Result<String> {
    let open = format!("[[{}:", map.prefix);
    let mut out = String::with_capacity(masked.len());
    let mut rest = masked;

    while let Some(at) = rest.find(&open) {
        out.push_str(&rest[..at]);
        let after = &rest[at + open.len()..];

        let close = after
            .find("]]")
            .ok_or_else(|| Error::restore_token(format!("{open}...  에 닫는 괄호가 없다")))?;

        let index: usize = after[..close]
            .parse()
            .map_err(|_| Error::restore_token(format!("토큰 번호가 숫자가 아니다: '{}'", &after[..close])))?;

        let entry = map
            .entries
            .get(index)
            .ok_or_else(|| Error::restore_token(format!("표에 없는 토큰 번호 {index}")))?;

        out.push_str(&entry.original);
        rest = &after[close + 2..];
    }

    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_text_gets_the_default_prefix() {
        let map = RestoreMap::for_text("주민등록번호 880101-1234568");
        assert_eq!(map.prefix(), "PII");
    }

    #[test]
    fn a_colliding_text_gets_a_different_prefix() {
        // 원문이 이미 토큰처럼 생긴 문자열을 갖고 있으면 접두어를 바꾼다.
        let map = RestoreMap::for_text("로그에 [[PII:0]] 가 남아 있다");
        assert_ne!(map.prefix(), "PII");
        assert!(!"로그에 [[PII:0]] 가 남아 있다".contains(&format!("[[{}:", map.prefix())));
    }

    #[test]
    fn repeated_collisions_keep_climbing() {
        let text = "[[PII:0]] [[PII0:1]] [[PII1:2]]";
        let map = RestoreMap::for_text(text);
        assert!(!text.contains(&format!("[[{}:", map.prefix())));
    }

    #[test]
    fn unmask_restores_byte_for_byte() {
        let mut map = RestoreMap::for_text("원문");
        let a = map.push(EntityKind::Resident, "880101-1234568");
        let b = map.push(EntityKind::Phone, "010-1234-5678");
        let masked = format!("주민 {a} 연락처 {b} 끝");
        assert_eq!(unmask(&masked, &map).unwrap(), "주민 880101-1234568 연락처 010-1234-5678 끝");
    }

    #[test]
    fn text_without_tokens_passes_through() {
        let map = RestoreMap::for_text("x");
        assert_eq!(unmask("아무 토큰도 없다", &map).unwrap(), "아무 토큰도 없다");
    }

    #[test]
    fn an_unknown_token_number_is_an_error() {
        let mut map = RestoreMap::for_text("x");
        map.push(EntityKind::Email, "a@b.com");
        let err = unmask("[[PII:7]]", &map).unwrap_err();
        assert!(matches!(err, Error::RestoreToken { .. }));
    }

    #[test]
    fn a_malformed_token_is_an_error() {
        let mut map = RestoreMap::for_text("x");
        map.push(EntityKind::Email, "a@b.com");
        assert!(unmask("[[PII:abc]]", &map).is_err());
        assert!(unmask("[[PII:0", &map).is_err());
    }
}
