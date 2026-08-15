//! 엔티티별 인식기.
//!
//! 숫자 런 하나를 받아 **어떤 엔티티일 수 있는지** 후보를 낸다. 한 런이 여러 후보를 낳는 것이
//! 정상이다. 열 자리 숫자열은 사업자등록번호일 수도, 지역 전화번호일 수도, 계좌번호일 수도
//! 있다. 어느 쪽인지는 검증식과 문맥이 정한다.
//!
//! 이 모듈은 판정하지 않는다. 후보와 그 후보의 검증 결과, 그리고 **그 엔티티가 도달할 수 있는
//! 최고 등급**까지만 낸다. 점수 계산과 최종 판정은 [`super`] 의 오케스트레이션이 한다.

use serde::{Deserialize, Serialize};

use super::checksum::{self, ChecksumResult};
use super::context::{self, Cue};

/// 이 라이브러리가 다루는 개인정보 종류.
///
/// 이 열거형만 `Deserialize` 도 유도한다. 필드가 없어 수명 문제가 없고, 복원 맵을 파일로
/// 저장했다가 다시 읽어야 마스킹의 가역성이 프로세스 경계를 넘어 성립하기 때문이다.
/// JSON 이름은 소문자 스네이크 케이스다. Python 바인딩과 명령줄 도구가 내는 이름,
/// 그리고 직렬화 결과가 서로 달라지면 같은 값에 이름이 둘 생긴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// 주민등록번호.
    Resident,
    /// 외국인등록번호.
    ForeignerRegistration,
    /// 사업자등록번호.
    BusinessRegistration,
    /// 신용카드 또는 체크카드 번호.
    CreditCard,
    /// 은행 계좌번호.
    BankAccount,
    /// 전화번호.
    Phone,
    /// 이메일 주소.
    Email,
    /// 운전면허번호.
    DriverLicense,
    /// 생년월일.
    BirthDate,
    /// 여권번호.
    Passport,
}

impl EntityKind {
    /// 사람이 읽는 한국어 이름. `"주민등록번호"`
    ///
    /// 한국어 문서를 다루는 한국어 사용자를 위한 이름이다. 영문 산출물에 쓸 이름은
    /// [`EntityKind::code_upper`] 다.
    pub fn label(&self) -> &'static str {
        match self {
            EntityKind::Resident => "주민등록번호",
            EntityKind::ForeignerRegistration => "외국인등록번호",
            EntityKind::BusinessRegistration => "사업자등록번호",
            EntityKind::CreditCard => "카드번호",
            EntityKind::BankAccount => "계좌번호",
            EntityKind::Phone => "전화번호",
            EntityKind::Email => "이메일",
            EntityKind::DriverLicense => "운전면허번호",
            EntityKind::BirthDate => "생년월일",
            EntityKind::Passport => "여권번호",
        }
    }

    /// 기계용 소문자 식별자. `"credit_card"`
    ///
    /// **이 이름 하나가 JSON 직렬화 이름이자 Python 속성값이자 명령줄 출력값이다.**
    /// 같은 값에 이름이 여럿 생기지 않도록 한 곳에서만 정한다. 직렬화 이름과 어긋나지
    /// 않는지는 단위 테스트가 검사한다.
    pub fn code(&self) -> &'static str {
        match self {
            EntityKind::Resident => "resident",
            EntityKind::ForeignerRegistration => "foreigner_registration",
            EntityKind::BusinessRegistration => "business_registration",
            EntityKind::CreditCard => "credit_card",
            EntityKind::BankAccount => "bank_account",
            EntityKind::Phone => "phone",
            EntityKind::Email => "email",
            EntityKind::DriverLicense => "driver_license",
            EntityKind::BirthDate => "birth_date",
            EntityKind::Passport => "passport",
        }
    }

    /// 영문 대문자 이름. `"CREDIT_CARD"`
    ///
    /// 한국어 자리표시자가 문서를 오염시키면 안 되는 경우를 위한 것이다. 한국어 문서를
    /// 다루는 비한국어 사용자가 실제 사용자층이고, 그들의 산출물에 `[카드번호]` 가 박히면
    /// 곤란하다. [`code`](EntityKind::code) 의 대문자형이라 새로 외울 이름이 늘지 않는다.
    pub fn code_upper(&self) -> &'static str {
        match self {
            EntityKind::Resident => "RESIDENT",
            EntityKind::ForeignerRegistration => "FOREIGNER_REGISTRATION",
            EntityKind::BusinessRegistration => "BUSINESS_REGISTRATION",
            EntityKind::CreditCard => "CREDIT_CARD",
            EntityKind::BankAccount => "BANK_ACCOUNT",
            EntityKind::Phone => "PHONE",
            EntityKind::Email => "EMAIL",
            EntityKind::DriverLicense => "DRIVER_LICENSE",
            EntityKind::BirthDate => "BIRTH_DATE",
            EntityKind::Passport => "PASSPORT",
        }
    }

    /// 소문자 식별자로 되찾는다. 모르는 이름이면 `None`.
    pub fn from_code(code: &str) -> Option<Self> {
        ALL.iter().copied().find(|kind| kind.code() == code)
    }
}

/// 이 라이브러리가 다루는 엔티티 전부. 새 엔티티를 넣으면 여기에도 넣어야 한다.
///
/// 목록 자체를 테스트가 검사하므로 빠뜨리면 걸린다.
pub const ALL: &[EntityKind] = &[
    EntityKind::Resident,
    EntityKind::ForeignerRegistration,
    EntityKind::BusinessRegistration,
    EntityKind::CreditCard,
    EntityKind::BankAccount,
    EntityKind::Phone,
    EntityKind::Email,
    EntityKind::DriverLicense,
    EntityKind::BirthDate,
    EntityKind::Passport,
];

/// 판정 등급.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// 형식만 맞다. 문맥 단서가 없다.
    Possible,
    /// 형식이 맞고 문맥 단서가 충분하다. 또는 검증식이 없는 엔티티다.
    Probable,
    /// 검증식을 통과했다.
    Certain,
}

/// 검증 후보 하나.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// 무슨 엔티티로 본 것인가.
    pub entity: EntityKind,
    /// 어떤 패턴 규칙이 걸렸는가. 판정 근거에 그대로 실린다.
    pub rule: &'static str,
    /// 검증식 결과.
    pub checksum: ChecksumResult,
    /// 이 엔티티가 도달할 수 있는 최고 등급.
    ///
    /// 검증식이 없거나 공개돼 있지 않은 엔티티는 아무리 문맥이 좋아도 `Certain` 이 될 수 없다.
    /// 그 사실을 감추지 않기 위해 타입으로 못박는다.
    pub ceiling: Certainty,
    /// 이 엔티티의 문맥 단서 사전.
    pub cues: &'static [Cue],
    /// 패턴 기본점.
    pub base: f32,
    /// 문맥 단서 없이도 성립하는가.
    ///
    /// 카드번호처럼 검증식이 강한 엔티티는 문맥이 없어도 된다. 계좌번호처럼 숫자열만으로는
    /// 아무것도 말할 수 없는 엔티티는 문맥이 필수다.
    pub needs_context: bool,
}

/// 서울 지역번호를 뺀 나머지 지역번호. 앞의 `0` 을 포함한 세 자리다.
const AREA_CODES: &[&str] = &[
    "031", "032", "033", "041", "042", "043", "044", "051", "052", "053", "054", "055", "061",
    "062", "063", "064", "070",
];

/// 휴대전화 식별번호.
const MOBILE_PREFIXES: &[&str] = &["010", "011", "016", "017", "018", "019"];

/// 대표번호 식별번호.
const REPRESENTATIVE_PREFIXES: &[&str] = &["15", "16", "18"];

/// 운전면허 지역 코드. 최초 발급 지방경찰청을 뜻한다.
const LICENSE_REGIONS: &[&str] = &[
    "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26",
    "28",
];

/// 숫자 런을 문자열로 되돌린다. 접두 비교에 쓴다.
///
/// 가려진 자리는 `*` 로 낸다. 그래서 `text[..3] == "010"` 같은 접두 검사가 마스킹된 값에서
/// 저절로 실패한다. 모르는 것을 아는 척하지 않는 쪽으로 기운다.
fn as_text(digits: &[u8]) -> String {
    digits
        .iter()
        .map(|&d| if d == checksum::MASKED { '*' } else { char::from(b'0' + d) })
        .collect()
}

/// 숫자 런 하나에서 가능한 모든 후보를 낸다.
pub fn candidates(digits: &[u8]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let text = as_text(digits);
    let len = digits.len();

    // 주민등록번호와 외국인등록번호. 성별코드가 둘을 가른다.
    if let Some(analysis) = checksum::analyze_resident(digits) {
        // 앞 6자리가 실재하는 날짜가 아니면 등록번호가 아니다. 이것만은 검증식보다 강한 제약이다.
        //
        // 예외가 하나 있다. 앞자리가 가려져 있으면 날짜를 확인할 방법 자체가 없다. 이때는
        // 후보에서 빼는 대신 **문맥을 필수로 걸고 기본점을 낮춰** 남긴다. 마스킹된 문서를
        // 다시 처리하는 경우가 실무에 있고, 날짜 제약을 못 쓰는 만큼 문맥이 그 자리를 메운다.
        if analysis.birth.is_none() && checksum::has_masked(digits) {
            let (entity, rule, cues) = if analysis.gender.foreigner {
                (EntityKind::ForeignerRegistration, "resident.foreigner_masked", context::FOREIGNER)
            } else {
                (EntityKind::Resident, "resident.korean_masked", context::RESIDENT)
            };
            out.push(Candidate {
                entity,
                rule,
                checksum: analysis.checksum,
                ceiling: Certainty::Probable,
                cues,
                base: 0.2,
                needs_context: true,
            });
        }

        if analysis.birth.is_some() {
            let (entity, rule, cues) = if analysis.gender.foreigner {
                (
                    EntityKind::ForeignerRegistration,
                    "resident.foreigner_13",
                    context::FOREIGNER,
                )
            } else {
                (EntityKind::Resident, "resident.korean_13", context::RESIDENT)
            };
            out.push(Candidate {
                entity,
                rule,
                checksum: analysis.checksum,
                // 가려진 자리가 있으면 검증식을 못 돌리므로 천장도 함께 내린다.
                ceiling: if checksum::has_masked(digits) {
                    Certainty::Probable
                } else {
                    Certainty::Certain
                },
                cues,
                base: 0.6,
                needs_context: false,
            });
        }
    }

    // 사업자등록번호.
    if len == 10 {
        out.push(Candidate {
            entity: EntityKind::BusinessRegistration,
            rule: "business.10",
            checksum: checksum::business_registration(digits),
            ceiling: Certainty::Certain,
            cues: context::BUSINESS,
            base: 0.4,
            needs_context: false,
        });
    }

    // 카드번호. 국제 표준 검증식이라 이 라이브러리에서 가장 확실한 엔티티다.
    if (13..=19).contains(&len) {
        out.push(Candidate {
            entity: EntityKind::CreditCard,
            rule: "card.luhn",
            checksum: checksum::luhn(digits),
            ceiling: Certainty::Certain,
            cues: context::CARD,
            base: 0.4,
            needs_context: false,
        });
    }

    // 전화번호. 식별번호로 형태를 가른다.
    //
    // 기본점이 0.6 인 이유는 문턱과의 여유 때문이다. 전화번호는 검증식이 없고 문맥도 필수가
    // 아니라, **기본점 하나로 문턱을 넘어야 하는 유일한 엔티티**다. 기본점을 문턱과 같은 0.5 로
    // 두면 하이픈 두 개(감점 0.02)나 전각 열한 자(감점 0.022)만 있어도 문턱 아래로 떨어져,
    // 한국에서 가장 흔한 표기인 `010-1234-5678` 이 문맥 없이는 안 잡힌다. 합성 코퍼스 실측에서
    // 전화번호 재현율이 66.7퍼센트로 나와 드러난 결함이고, 여유 0.1 을 줘서 고쳤다.
    //
    // 일반화하면 이렇다. **문맥이 필수가 아니고 검증식도 없는 엔티티의 기본점은 문턱보다
    // 정규화 비용의 통상 범위만큼 높아야 한다.** 같으면 정규화가 판정을 뒤집는다.
    if let Some(rule) = phone_rule(&text) {
        out.push(Candidate {
            entity: EntityKind::Phone,
            rule,
            checksum: ChecksumResult::NotApplicable("전화번호에는 검증식이 없다"),
            ceiling: Certainty::Probable,
            cues: context::PHONE,
            base: 0.6,
            needs_context: false,
        });
    }

    // 운전면허번호. 검증 자릿수가 존재하지만 계산식이 공개돼 있지 않아 형식만 본다.
    if len == 12 && LICENSE_REGIONS.contains(&&text[..2]) {
        out.push(Candidate {
            entity: EntityKind::DriverLicense,
            rule: "license.12",
            checksum: ChecksumResult::NotApplicable(
                "검증 자릿수는 존재하나 계산식이 공개돼 있지 않다",
            ),
            ceiling: Certainty::Probable,
            cues: context::DRIVER_LICENSE,
            base: 0.2,
            needs_context: true,
        });
    }

    // 계좌번호. 숫자열만으로는 아무것도 말할 수 없어 문맥이 필수다.
    if (10..=14).contains(&len) {
        out.push(Candidate {
            entity: EntityKind::BankAccount,
            rule: "account.length",
            checksum: ChecksumResult::NotApplicable(
                "은행별 체계가 달라 공통 검증식이 없다",
            ),
            ceiling: Certainty::Probable,
            cues: context::ACCOUNT,
            base: 0.1,
            needs_context: true,
        });
    }

    // 생년월일. 일반 날짜와 구분되지 않으므로 문맥이 필수다.
    if let Some(rule) = birth_date_rule(digits) {
        out.push(Candidate {
            entity: EntityKind::BirthDate,
            rule,
            checksum: ChecksumResult::NotApplicable("생년월일에는 검증식이 없다"),
            ceiling: Certainty::Probable,
            cues: context::BIRTH_DATE,
            base: 0.1,
            needs_context: true,
        });
    }

    out
}

/// 여권번호 후보. 모양은 [`super::scanner::passports`] 가 이미 가렸다.
///
/// 숫자 런이 아니라 영숫자 토큰에서 나오므로 [`candidates`] 와 별도 입구다.
///
/// 문맥이 필수이고 기본점이 바닥인 이유는 같다. **여권번호에는 공개된 검증식이 없고 모양이
/// 흔하다.** `A1234567` 은 주문 번호이기도 하고 부품 코드이기도 하다. 형식만으로 개인정보라고
/// 부르면 문서 하나에서 오탐이 수십 건 난다. 그래서 판정을 전부 문맥에 맡긴다.
pub fn passport_candidate() -> Candidate {
    Candidate {
        entity: EntityKind::Passport,
        rule: "passport.shape",
        checksum: ChecksumResult::NotApplicable("여권번호에는 공개된 검증식이 없다"),
        ceiling: Certainty::Probable,
        cues: context::PASSPORT,
        base: 0.1,
        needs_context: true,
    }
}

/// 전화번호 형태를 가른다. 아니면 `None`.
fn phone_rule(text: &str) -> Option<&'static str> {
    let len = text.len();

    if len == 11 && MOBILE_PREFIXES.contains(&&text[..3]) {
        return Some("phone.mobile_11");
    }
    if len == 10 && MOBILE_PREFIXES[1..].contains(&&text[..3]) {
        // 010 은 여덟 자리 가입번호가 없다. 011 계열의 옛 번호만 열 자리다.
        return Some("phone.mobile_10");
    }
    if (9..=10).contains(&len) && text.starts_with("02") {
        return Some("phone.seoul");
    }
    if (10..=11).contains(&len) && AREA_CODES.contains(&&text[..3]) {
        return Some("phone.landline");
    }
    if len == 8 && REPRESENTATIVE_PREFIXES.contains(&&text[..2]) {
        return Some("phone.representative");
    }
    None
}

/// 생년월일 형태를 가른다. 여섯 자리와 여덟 자리를 본다.
///
/// 생년월일은 검증식이 없고 날짜 유효성이 유일한 제약이라, 가려진 자리가 있으면 그 제약이
/// 사라진다. 남는 것이 자릿수뿐이면 일반 숫자와 구분되지 않으므로 후보에서 뺀다.
fn birth_date_rule(digits: &[u8]) -> Option<&'static str> {
    if checksum::has_masked(digits) {
        return None;
    }
    match digits.len() {
        6 => {
            let yy = u16::from(digits[0]) * 10 + u16::from(digits[1]);
            let month = digits[2] * 10 + digits[3];
            let day = digits[4] * 10 + digits[5];
            // 두 자리 연도는 세기를 알 수 없다. 윤년 판정에 영향을 주는 2월 29일만 조심하면
            // 되는데, 1900년대와 2000년대 어느 쪽으로도 읽힐 수 있으므로 관대하게 본다.
            (checksum::is_valid_date(1900 + yy, month, day)
                || checksum::is_valid_date(2000 + yy, month, day))
            .then_some("birth.yymmdd")
        }
        8 => {
            let year = u16::from(digits[0]) * 1000
                + u16::from(digits[1]) * 100
                + u16::from(digits[2]) * 10
                + u16::from(digits[3]);
            let month = digits[4] * 10 + digits[5];
            let day = digits[6] * 10 + digits[7];
            ((1800..=2200).contains(&year) && checksum::is_valid_date(year, month, day))
                .then_some("birth.yyyymmdd")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(s: &str) -> Vec<u8> {
        checksum::to_digits(s).unwrap()
    }

    /// 한 값에 이름이 여럿 생기지 않게 고정한다.
    ///
    /// `code()` 는 손으로 적은 표이고 직렬화 이름은 파생 매크로가 만든다. 둘이 갈라지면
    /// JSON 과 Python 속성이 서로 다른 이름을 내게 된다. 실제로 한 번 그렇게 됐었다.
    #[test]
    fn code_matches_the_serialized_name_for_every_entity() {
        for kind in ALL {
            let json = serde_json::to_string(kind).unwrap();
            let serialized = json.trim_matches('"');
            assert_eq!(kind.code(), serialized, "{kind:?} 의 code 와 직렬화 이름이 다르다");
        }
    }

    #[test]
    fn code_upper_is_the_uppercase_of_code() {
        for kind in ALL {
            assert_eq!(
                kind.code_upper(),
                kind.code().to_uppercase(),
                "{kind:?} 의 두 이름이 갈라졌다"
            );
        }
    }

    #[test]
    fn every_entity_has_all_three_names_and_round_trips() {
        for kind in ALL {
            assert!(!kind.label().is_empty());
            assert!(kind.code_upper().is_ascii(), "{kind:?} 의 영문 이름에 비아스키가 있다");
            assert_eq!(EntityKind::from_code(kind.code()), Some(*kind));
        }
        assert_eq!(EntityKind::from_code("nope"), None);
    }

    #[test]
    fn the_all_list_has_no_duplicates() {
        let mut codes: Vec<&str> = ALL.iter().map(|k| k.code()).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "ALL 에 중복이 있다");
    }

    fn kinds(s: &str) -> Vec<EntityKind> {
        candidates(&digits(s)).iter().map(|c| c.entity).collect()
    }

    #[test]
    fn thirteen_digits_with_valid_date_is_a_resident_number() {
        let kinds = kinds("8801011234567");
        assert!(kinds.contains(&EntityKind::Resident));
        assert!(!kinds.contains(&EntityKind::ForeignerRegistration));
    }

    #[test]
    fn gender_code_five_makes_it_a_foreigner_number() {
        let kinds = kinds("8801015234567");
        assert!(kinds.contains(&EntityKind::ForeignerRegistration));
        assert!(!kinds.contains(&EntityKind::Resident));
    }

    #[test]
    fn thirteen_digits_with_impossible_date_is_not_a_resident_number() {
        // 13월 32일. 검증식 이전에 날짜가 아니다.
        let kinds = kinds("8813321234567");
        assert!(!kinds.contains(&EntityKind::Resident));
        assert!(!kinds.contains(&EntityKind::ForeignerRegistration));
    }

    #[test]
    fn thirteen_digits_can_still_be_a_card() {
        // 같은 런이 여러 후보를 낳는 것이 정상이다.
        assert!(kinds("8801011234567").contains(&EntityKind::CreditCard));
    }

    #[test]
    fn ten_digits_yield_business_account_and_maybe_phone() {
        let kinds = kinds("1248100998");
        assert!(kinds.contains(&EntityKind::BusinessRegistration));
        assert!(kinds.contains(&EntityKind::BankAccount));
    }

    #[test]
    fn business_checksum_is_carried_on_the_candidate() {
        let cs = candidates(&digits("1248100998"));
        let business = cs
            .iter()
            .find(|c| c.entity == EntityKind::BusinessRegistration)
            .unwrap();
        assert!(business.checksum.passed());
        assert_eq!(business.ceiling, Certainty::Certain);
    }

    #[test]
    fn phone_shapes_are_classified() {
        assert_eq!(phone_rule("01012345678"), Some("phone.mobile_11"));
        assert_eq!(phone_rule("0111234567"), Some("phone.mobile_10"));
        assert_eq!(phone_rule("0212345678"), Some("phone.seoul"));
        assert_eq!(phone_rule("021234567"), Some("phone.seoul"));
        assert_eq!(phone_rule("0311234567"), Some("phone.landline"));
        assert_eq!(phone_rule("07012345678"), Some("phone.landline"));
        assert_eq!(phone_rule("15881234"), Some("phone.representative"));
    }

    #[test]
    fn non_phone_shapes_are_rejected() {
        assert_eq!(phone_rule("0101234567"), None, "010 은 열 자리가 없다");
        assert_eq!(phone_rule("0991234567"), None, "099 는 지역번호가 아니다");
        assert_eq!(phone_rule("12345678"), None);
    }

    #[test]
    fn driver_license_needs_a_known_region_code() {
        assert!(kinds("111234567890").contains(&EntityKind::DriverLicense));
        assert!(!kinds("991234567890").contains(&EntityKind::DriverLicense));
    }

    #[test]
    fn driver_license_can_never_reach_certain() {
        let cs = candidates(&digits("111234567890"));
        let license = cs
            .iter()
            .find(|c| c.entity == EntityKind::DriverLicense)
            .unwrap();
        assert_eq!(license.ceiling, Certainty::Probable);
        assert!(matches!(license.checksum, ChecksumResult::NotApplicable(_)));
    }

    #[test]
    fn account_and_license_need_context() {
        for (number, entity) in [
            ("1234567890123", EntityKind::BankAccount),
            ("111234567890", EntityKind::DriverLicense),
        ] {
            let cs = candidates(&digits(number));
            let found = cs.iter().find(|c| c.entity == entity).unwrap();
            assert!(found.needs_context, "{} 는 문맥이 필수다", entity.label());
        }
    }

    #[test]
    fn birth_dates_are_recognized() {
        assert_eq!(birth_date_rule(&digits("880101")), Some("birth.yymmdd"));
        assert_eq!(birth_date_rule(&digits("19880101")), Some("birth.yyyymmdd"));
        assert_eq!(birth_date_rule(&digits("881301")), None, "13월은 없다");
        assert_eq!(birth_date_rule(&digits("880230")), None, "2월 30일은 없다");
        assert_eq!(birth_date_rule(&digits("000229")), Some("birth.yymmdd"), "2000년은 윤년");
    }

    #[test]
    fn plain_number_yields_only_weak_candidates() {
        // 주문번호처럼 아무 의미 없는 열두 자리.
        let cs = candidates(&digits("987654321098"));
        assert!(
            cs.iter().all(|c| c.needs_context || c.ceiling == Certainty::Probable),
            "약한 후보만 나와야 한다"
        );
    }
}
