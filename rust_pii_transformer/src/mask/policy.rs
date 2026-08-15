//! 마스킹 정책.
//!
//! 정책은 엔티티별로 다르게 줄 수 있다. 주민등록번호는 통째로 가리고 전화번호는 뒷자리만 남기는
//! 조합이 실무에서 흔하다.

use crate::detect::EntityKind;

/// 전체 치환을 어떤 모양으로 할 것인가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redaction {
    /// **한국어** 이름을 대괄호로 감싼다. `[주민등록번호]`
    ///
    /// 무엇이 지워졌는지가 남아서 사람이 읽기 좋다. 대신 그 자리에 무엇이 있었는지의 종류가
    /// 노출되므로, 종류조차 숨겨야 하면 [`Redaction::Fixed`] 를 쓴다.
    ///
    /// 산출물이 한국어가 아니면 [`Redaction::Code`] 를 쓴다.
    Label,
    /// **영문 대문자** 이름을 대괄호로 감싼다. `[CREDIT_CARD]`
    ///
    /// 한국어 문서를 다루는 비한국어 사용자를 위한 것이다. 이 라이브러리는 한국어 특화지만
    /// 쓰는 사람까지 한국어 사용자인 것은 아니다. 영문 보고서나 로그에 `[카드번호]` 가 박히면
    /// 문서가 오염된다.
    Code,
    /// 원문 **문자 수**만큼 같은 문자를 반복한다. `*************`
    ///
    /// 자릿수가 보존되므로 표 정렬이 깨지지 않는다. 대신 자릿수 자체가 정보라는 점은 남는다.
    Fill(char),
    /// 고정 문자열로 바꾼다. 길이도 종류도 남기지 않는다.
    Fixed(String),
}

/// 마스킹 정책 4종.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    /// 전체 치환. 불가역.
    Redact(Redaction),
    /// 부분 노출. 앞뒤 일부만 남기고 가운데를 덮는다. 불가역.
    ///
    /// 자릿수는 **문자** 기준이다. 한글이 섞여도 바이트가 아니라 글자로 센다.
    /// `keep_prefix + keep_suffix` 가 원문 문자 수 이상이면 아무것도 남기지 않는다.
    /// 짧은 값에서 전체가 그대로 노출되는 사고를 막기 위해서다.
    Partial {
        /// 앞에서 남길 문자 수.
        keep_prefix: usize,
        /// 뒤에서 남길 문자 수.
        keep_suffix: usize,
        /// 가운데를 덮을 문자.
        fill: char,
    },
    /// 해시 가명화. 불가역이지만 **같은 값은 항상 같은 토큰**이 되어 연결성이 유지된다.
    ///
    /// 같은 사람의 기록을 세거나 묶는 통계 분석이 가능하다. 열쇠가 없으면 되돌릴 수 없고,
    /// 열쇠가 있어도 원본 공간을 전수로 훑어야 하므로 자릿수가 짧은 엔티티는 사실상 되맞출 수
    /// 있다는 점을 알고 써야 한다. 주민등록번호처럼 후보 공간이 좁은 값은 해시만으로 안전하지 않다.
    ///
    /// `hash` 기능 플래그를 켰을 때만 존재한다.
    #[cfg(feature = "hash")]
    Hash {
        /// 해시 기반 메시지 인증 코드(HMAC)의 열쇠. 이 값이 새면 가명화가 무너진다.
        key: Vec<u8>,
        /// 남길 16진수 자릿수.
        len: usize,
    },
    /// 토큰화. **완전 가역.** 복원 맵을 함께 낸다.
    Tokenize,
}

impl Default for Policy {
    fn default() -> Self {
        Policy::Redact(Redaction::Label)
    }
}

/// 엔티티별 정책 묶음.
///
/// 기본 정책 하나에 엔티티별 예외를 얹는 구조다. 예외가 없으면 기본이 적용된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySet {
    default: Policy,
    overrides: Vec<(EntityKind, Policy)>,
}

impl PolicySet {
    /// 모든 엔티티에 같은 정책을 적용한다.
    pub fn new(default: Policy) -> Self {
        Self { default, overrides: Vec::new() }
    }

    /// 특정 엔티티에만 다른 정책을 준다. 같은 엔티티를 두 번 주면 나중 것이 이긴다.
    pub fn with(mut self, entity: EntityKind, policy: Policy) -> Self {
        if let Some(slot) = self.overrides.iter_mut().find(|(e, _)| *e == entity) {
            slot.1 = policy;
        } else {
            self.overrides.push((entity, policy));
        }
        self
    }

    /// 이 엔티티에 적용할 정책.
    pub fn policy_for(&self, entity: EntityKind) -> &Policy {
        self.overrides
            .iter()
            .find(|(e, _)| *e == entity)
            .map(|(_, p)| p)
            .unwrap_or(&self.default)
    }

    /// 토큰화 정책이 하나라도 있는가. 복원 맵을 만들지 결정한다.
    pub(crate) fn uses_tokenize(&self) -> bool {
        matches!(self.default, Policy::Tokenize)
            || self.overrides.iter().any(|(_, p)| matches!(p, Policy::Tokenize))
    }
}

impl Default for PolicySet {
    /// 기본은 엔티티 이름을 남기는 전체 치환이다.
    fn default() -> Self {
        Self::new(Policy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_beat_the_default() {
        let set = PolicySet::new(Policy::Redact(Redaction::Label)).with(
            EntityKind::Phone,
            Policy::Partial { keep_prefix: 3, keep_suffix: 4, fill: '*' },
        );
        assert!(matches!(set.policy_for(EntityKind::Resident), Policy::Redact(_)));
        assert!(matches!(set.policy_for(EntityKind::Phone), Policy::Partial { .. }));
    }

    #[test]
    fn the_same_entity_can_be_reassigned() {
        let set = PolicySet::default()
            .with(EntityKind::Phone, Policy::Tokenize)
            .with(EntityKind::Phone, Policy::Redact(Redaction::Fill('*')));
        assert!(matches!(set.policy_for(EntityKind::Phone), Policy::Redact(_)));
        assert_eq!(set.overrides.len(), 1, "같은 엔티티가 두 번 쌓이면 안 된다");
    }

    #[test]
    fn tokenize_is_detected_in_either_slot() {
        assert!(!PolicySet::default().uses_tokenize());
        assert!(PolicySet::new(Policy::Tokenize).uses_tokenize());
        assert!(PolicySet::default().with(EntityKind::Email, Policy::Tokenize).uses_tokenize());
    }
}
