//! 엔티티별 검증식.
//!
//! 이 모듈에는 **근거가 확인된 계산만** 들어간다. 각 함수의 문서에 근거와 그 근거의 강도를
//! 함께 적었다. 확인되지 않은 알고리즘은 추측으로 구현하지 않고 아예 만들지 않는다.
//! 운전면허번호가 그렇다. 검증 자릿수가 존재한다는 것은 알려져 있지만 계산식이 공개돼 있지
//! 않아 이 모듈에 함수가 없다. 형식 검사만 하고 최고 등급을 `Probable` 로 제한한다.

use serde::Serialize;

/// 검증식 적용 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumResult {
    /// 검증식을 통과했다.
    Passed,
    /// 검증식에서 떨어졌다.
    Failed,
    /// 이 엔티티에 검증식이 없거나 이 값에는 적용되지 않는다. 사유를 함께 담는다.
    NotApplicable(&'static str),
}

impl ChecksumResult {
    /// 통과했는가. `NotApplicable` 은 통과가 아니다.
    pub fn passed(&self) -> bool {
        matches!(self, ChecksumResult::Passed)
    }

    /// 명시적으로 떨어졌는가. 후보를 버릴지 판단하는 데 쓴다.
    pub fn failed(&self) -> bool {
        matches!(self, ChecksumResult::Failed)
    }
}

/// 2020년 10월 개편 이후 발급분에 검증식이 성립하지 않는다는 사유 문구.
pub const RRN_RANDOMIZED: &str =
    "2020년 10월 이후 발급분은 뒷자리가 임의번호라 검증식이 성립하지 않는다";

/// 주민등록번호와 외국인등록번호 공통 가중치.
const RESIDENT_WEIGHTS: [u32; 12] = [2, 3, 4, 5, 6, 7, 8, 9, 2, 3, 4, 5];

/// 사업자등록번호 가중치.
const BUSINESS_WEIGHTS: [u32; 9] = [1, 3, 7, 1, 3, 7, 1, 3, 5];

/// 성별코드가 가리키는 출생 세기와 국적 구분.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenderCode {
    /// 출생 연도의 세기. 1800, 1900, 2000 중 하나.
    pub century: u16,
    /// 외국인등록번호인가.
    pub foreigner: bool,
}

/// 성별코드 한 자리를 해석한다. 범위 밖이면 `None`.
///
/// 근거: 1,2 는 1900년대 내국인, 3,4 는 2000년대 내국인, 5,6 은 1900년대 외국인,
/// 7,8 은 2000년대 외국인, 9,0 은 1800년대다.
pub fn gender_code(digit: u8) -> Option<GenderCode> {
    let (century, foreigner) = match digit {
        1 | 2 => (1900, false),
        3 | 4 => (2000, false),
        5 | 6 => (1900, true),
        7 | 8 => (2000, true),
        9 | 0 => (1800, false),
        _ => return None,
    };
    Some(GenderCode { century, foreigner })
}

/// 주민등록번호와 외국인등록번호를 함께 해석한 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentAnalysis {
    /// 성별코드 해석 결과.
    pub gender: GenderCode,
    /// 앞 6자리가 실재하는 날짜이면 그 값. 아니면 `None`.
    pub birth: Option<(u16, u8, u8)>,
    /// 검증식 결과.
    pub checksum: ChecksumResult,
}

/// 13자리 주민등록번호 또는 외국인등록번호를 해석한다.
///
/// 자릿수가 13이 아니거나 성별코드가 범위 밖이면 `None` 이다. 그 둘은 형식 자체가 아니라는
/// 뜻이므로 후보에서 뺀다.
///
/// ## 검증식의 근거와 그 한계
///
/// 내국인은 `(11 - (가중치 곱의 합 mod 11)) mod 10` 이 마지막 자리와 같아야 한다.
/// 외국인은 같은 식에 **2를 더한 뒤 다시 10으로 나눈 나머지**를 쓴다.
///
/// **검증식이 떨어져도 주민등록번호가 아니라고 단정할 수 없다.** 2020년 10월 개편으로
/// 뒷자리 여섯 자리가 임의번호가 되면서 그 이후 발급분에는 검증식이 성립하지 않기 때문이다.
/// 번호 자체만으로는 발급 시점을 알 수 없으므로, 검증 실패는 [`ChecksumResult::Failed`] 로
/// 내되 탐지 층이 그것을 버리는 근거가 아니라 등급을 낮추는 근거로 쓴다.
pub fn analyze_resident(digits: &[u8]) -> Option<ResidentAnalysis> {
    if digits.len() != 13 {
        return None;
    }

    // 성별코드가 가려져 있으면 세기도 내국인 여부도 알 수 없다. 후보에서 빼는 대신
    // 1900년대 내국인으로 가정하고, 아래에서 두 세기를 모두 시험해 날짜를 판정한다.
    let gender = match gender_code(digits[6]) {
        Some(g) => g,
        None if digits[6] == MASKED => GenderCode { century: 1900, foreigner: false },
        None => return None,
    };

    // 앞 6자리에 가려진 자리가 있으면 날짜를 확인할 방법이 없다.
    let birth = if has_masked(&digits[..6]) {
        None
    } else {
        let yy = u16::from(digits[0]) * 10 + u16::from(digits[1]);
        let mm = digits[2] * 10 + digits[3];
        let dd = digits[4] * 10 + digits[5];
        if digits[6] == MASKED {
            // 세기를 모르므로 둘 중 하나라도 실재하면 인정한다.
            [1900u16, 2000]
                .into_iter()
                .map(|c| c + yy)
                .find(|&year| is_valid_date(year, mm, dd))
                .map(|year| (year, mm, dd))
        } else {
            let year = gender.century + yy;
            is_valid_date(year, mm, dd).then_some((year, mm, dd))
        }
    };

    // 가려진 자리가 하나라도 있으면 검증식을 돌릴 수 없다.
    if has_masked(digits) {
        return Some(ResidentAnalysis {
            gender,
            birth,
            checksum: ChecksumResult::NotApplicable("가려진 자리가 있어 검증식을 적용할 수 없다"),
        });
    }

    let sum: u32 = digits[..12]
        .iter()
        .zip(RESIDENT_WEIGHTS)
        .map(|(d, w)| u32::from(*d) * w)
        .sum();
    let base = (11 - (sum % 11)) % 10;
    let expected = if gender.foreigner {
        (base + 2) % 10
    } else {
        base
    };

    let checksum = if u32::from(digits[12]) == expected {
        ChecksumResult::Passed
    } else {
        ChecksumResult::Failed
    };

    Some(ResidentAnalysis {
        gender,
        birth,
        checksum,
    })
}

/// 10자리 사업자등록번호를 검증한다.
///
/// ## 근거의 강도를 정직하게 적는다
///
/// 이 계산식은 **공식 문서에 공개돼 있지 않다.** 국세청과 국민신문고의 안내는 "전산시스템에
/// 의하여 오류 여부를 검증하기 위하여 1자리의 검증번호를 부여한다"까지만 말하고 계산 방법은
/// 밝히지 않는다. 여기 구현한 것은 여러 독립 구현체가 공통으로 쓰는 방식이고, 실재하는
/// 사업자등록번호 표본으로 실측해 확인했다. 무작위 10자리의 통과율이 약 10퍼센트로 관측되어
/// 모듈로 10 검증식이 실제로 존재한다는 점도 함께 확인됐다.
///
/// 계산은 앞 9자리에 가중치를 곱해 더하고, **아홉 번째 자리 곱의 십의 자리를 한 번 더 더한 뒤**,
/// `(10 - 합 mod 10) mod 10` 이 마지막 자리와 같은지 본다.
pub fn business_registration(digits: &[u8]) -> ChecksumResult {
    if digits.len() != 10 {
        return ChecksumResult::NotApplicable("사업자등록번호는 10자리다");
    }
    if has_masked(digits) {
        return ChecksumResult::NotApplicable("가려진 자리가 있어 검증식을 적용할 수 없다");
    }
    let mut sum: u32 = digits[..9]
        .iter()
        .zip(BUSINESS_WEIGHTS)
        .map(|(d, w)| u32::from(*d) * w)
        .sum();
    // 아홉 번째 자리만 곱의 십의 자리를 추가로 반영한다.
    sum += u32::from(digits[8]) * 5 / 10;

    if (10 - sum % 10) % 10 == u32::from(digits[9]) {
        ChecksumResult::Passed
    } else {
        ChecksumResult::Failed
    }
}

/// 카드번호 Luhn 검증.
///
/// 이 라이브러리가 다루는 엔티티 중 **유일하게 국제 표준(ISO/IEC 7812)으로 공개된 검증식**이다.
/// 그래서 카드번호만 다른 조건 없이 `Certain` 등급에 도달할 수 있다.
pub fn luhn(digits: &[u8]) -> ChecksumResult {
    if !(13..=19).contains(&digits.len()) {
        return ChecksumResult::NotApplicable("카드번호는 13자리에서 19자리다");
    }
    if has_masked(digits) {
        return ChecksumResult::NotApplicable("가려진 자리가 있어 검증식을 적용할 수 없다");
    }
    let mut sum = 0u32;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = u32::from(*d);
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    if sum % 10 == 0 {
        ChecksumResult::Passed
    } else {
        ChecksumResult::Failed
    }
}

/// 그해 그달의 날수. 그레고리력 윤년 규칙을 그대로 쓴다.
fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// 실재하는 날짜인가.
pub fn is_valid_date(year: u16, month: u8, day: u8) -> bool {
    (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

/// 마스킹된 자리를 나타내는 자릿값.
///
/// 0에서 9 범위 밖이라 산술에 섞여 들어가도 값이 오염되지 않고, 검사 한 번으로 걸러진다.
/// `900513-1****67` 처럼 **이미 부분 마스킹된 문서를 다시 처리**할 때 필요하다. 그런 텍스트를
/// 숫자가 아니라는 이유로 버리면 개인정보가 그대로 통과한다.
pub const MASKED: u8 = 10;

/// 마스킹된 자리가 하나라도 있는가.
pub fn has_masked(digits: &[u8]) -> bool {
    digits.contains(&MASKED)
}

/// 마스킹 문자인가. `*`, `X`, `x` 를 받는다.
fn is_mask_char(b: u8) -> bool {
    matches!(b, b'*' | b'X' | b'x')
}

/// 문자열의 각 문자를 자릿값으로 바꾼다. 숫자도 마스킹 문자도 아니면 `None`.
///
/// 마스킹 문자는 [`MASKED`] 로 들어간다. 검증식은 그 값을 만나면 판정을 포기하고
/// [`ChecksumResult::NotApplicable`] 을 내며, 그러면 등급이 `Certain` 에 도달하지 못한다.
/// **모른다는 사실이 등급으로 표현되는 것**이 이 설계의 요점이다.
///
/// `then_some` 이 아니라 `then` 을 쓴다. 전자는 인자를 즉시 평가하므로 숫자가 아닌 바이트에서
/// 뺄셈이 음수로 넘쳐 패닉한다.
pub fn to_digits(text: &str) -> Option<Vec<u8>> {
    text.bytes()
        .map(|b| {
            if is_mask_char(b) {
                Some(MASKED)
            } else {
                b.is_ascii_digit().then(|| b - b'0')
            }
        })
        .collect()
}

/// 실제 숫자(마스킹이 아닌) 자리의 개수.
pub fn known_count(digits: &[u8]) -> usize {
    digits.iter().filter(|&&d| d != MASKED).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(s: &str) -> Vec<u8> {
        to_digits(s).unwrap()
    }

    /// 검증식이 실제로 존재하는지 확인하는 통계적 성질.
    ///
    /// 모듈로 10 검증식이 있으면 무작위 번호의 통과율이 10퍼센트 근처여야 한다. 검증식이
    /// 없거나 구현이 틀리면 이 값이 크게 벗어난다. 고정 시드 선형 합동 생성기를 쓴다.
    fn random_pass_rate(len: usize, check: impl Fn(&[u8]) -> bool) -> f64 {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) % 10) as u8
        };
        let rounds = 20_000;
        let mut passed = 0;
        for _ in 0..rounds {
            let candidate: Vec<u8> = (0..len).map(|_| next()).collect();
            if check(&candidate) {
                passed += 1;
            }
        }
        f64::from(passed) / f64::from(rounds)
    }

    #[test]
    fn business_registration_accepts_real_numbers() {
        // 공개된 실재 사업자등록번호 표본이다. 알고리즘이 공식 문서에 없으므로 이 표본이
        // 근거의 자리를 대신한다.
        for number in [
            "1248100998", // 삼성전자
            "2208162517", // 네이버
            "1208147521", // 카카오
            "1078614075", // LG전자
            "1048137225", // SK텔레콤
            "1018109147", // 현대자동차
            "1108128774", // 포스코
        ] {
            assert!(
                business_registration(&digits(number)).passed(),
                "실재 사업자등록번호가 떨어졌다: {number}"
            );
        }
    }

    #[test]
    fn business_registration_rejects_tampered_digit() {
        // 마지막 자리를 하나 바꾸면 반드시 떨어져야 한다.
        let mut d = digits("1248100998");
        d[9] = (d[9] + 1) % 10;
        assert!(business_registration(&d).failed());
    }

    #[test]
    fn business_registration_has_a_real_check_digit() {
        let rate = random_pass_rate(10, |d| business_registration(d).passed());
        assert!(
            (0.08..0.12).contains(&rate),
            "모듈로 10 검증식이면 통과율이 10퍼센트 근처여야 한다. 실측 {rate}"
        );
    }

    #[test]
    fn business_registration_rejects_wrong_length() {
        assert!(matches!(
            business_registration(&digits("123456789")),
            ChecksumResult::NotApplicable(_)
        ));
    }

    #[test]
    fn luhn_accepts_standard_test_numbers() {
        // 카드사가 공개한 시험용 번호다. 실재 계좌에 연결되지 않는다.
        for number in [
            "4111111111111111", // Visa
            "5500005555555559", // Mastercard
            "378282246310005",  // American Express
            "6011111111111117", // Discover
        ] {
            assert!(luhn(&digits(number)).passed(), "시험 번호가 떨어졌다: {number}");
        }
    }

    #[test]
    fn luhn_rejects_tampered_digit() {
        assert!(luhn(&digits("4111111111111112")).failed());
    }

    #[test]
    fn luhn_has_a_real_check_digit() {
        let rate = random_pass_rate(16, |d| luhn(d).passed());
        assert!((0.08..0.12).contains(&rate), "실측 {rate}");
    }

    #[test]
    fn resident_reads_gender_code() {
        assert_eq!(
            gender_code(1),
            Some(GenderCode { century: 1900, foreigner: false })
        );
        assert_eq!(
            gender_code(4),
            Some(GenderCode { century: 2000, foreigner: false })
        );
        assert_eq!(
            gender_code(6),
            Some(GenderCode { century: 1900, foreigner: true })
        );
        assert_eq!(
            gender_code(8),
            Some(GenderCode { century: 2000, foreigner: true })
        );
        assert_eq!(
            gender_code(0),
            Some(GenderCode { century: 1800, foreigner: false })
        );
    }

    /// 검증식을 만족하는 13자리를 만들어 낸다. 앞 12자리는 시험용 고정값이다.
    fn make_resident(prefix: &str, foreigner: bool) -> Vec<u8> {
        let mut d = digits(prefix);
        assert_eq!(d.len(), 12);
        let sum: u32 = d
            .iter()
            .zip(RESIDENT_WEIGHTS)
            .map(|(x, w)| u32::from(*x) * w)
            .sum();
        let base = (11 - (sum % 11)) % 10;
        let check = if foreigner { (base + 2) % 10 } else { base };
        d.push(check as u8);
        d
    }

    #[test]
    fn resident_checksum_round_trips() {
        let korean = make_resident("880101112345", false);
        let analysis = analyze_resident(&korean).unwrap();
        assert!(analysis.checksum.passed());
        assert!(!analysis.gender.foreigner);
        assert_eq!(analysis.birth, Some((1988, 1, 1)));
    }

    #[test]
    fn foreigner_checksum_uses_the_plus_two_variant() {
        let foreigner = make_resident("880101512345", true);
        let analysis = analyze_resident(&foreigner).unwrap();
        assert!(analysis.checksum.passed());
        assert!(analysis.gender.foreigner);

        // 내국인 식으로 계산하면 떨어져야 한다. 두 식이 실제로 다르다는 확인이다.
        let wrong = make_resident("880101512345", false);
        assert!(analyze_resident(&wrong).unwrap().checksum.failed());
    }

    #[test]
    fn resident_rejects_impossible_date() {
        // 13월 32일. 검증식은 통과하더라도 날짜가 없으므로 birth 가 None 이다.
        let d = make_resident("881332112345", false);
        let analysis = analyze_resident(&d).unwrap();
        assert!(analysis.checksum.passed());
        assert_eq!(analysis.birth, None);
    }

    #[test]
    fn resident_rejects_out_of_range_gender_code() {
        // 성별코드는 0에서 8까지다. 9는 1800년대라 유효하고, 그 밖은 없다.
        let mut d = make_resident("880101112345", false);
        d[6] = 9;
        assert!(analyze_resident(&d).is_some(), "9는 1800년대라 유효하다");

        assert_eq!(analyze_resident(&digits("123456789012")), None, "12자리는 형식이 아니다");
    }

    #[test]
    fn resident_checksum_can_fail_on_a_real_number() {
        // 2020년 10월 이후 발급분은 검증식이 성립하지 않는다. 그 상황을 흉내 낸 값이다.
        // 탐지 층은 이것을 버리지 않고 등급만 낮춘다.
        let mut d = make_resident("880101112345", false);
        d[12] = (d[12] + 1) % 10;
        let analysis = analyze_resident(&d).unwrap();
        assert!(analysis.checksum.failed());
        assert_eq!(analysis.birth, Some((1988, 1, 1)), "날짜는 여전히 유효하다");
    }

    #[test]
    fn leap_year_rules_follow_the_gregorian_calendar() {
        assert!(is_valid_date(2024, 2, 29), "2024는 윤년");
        assert!(!is_valid_date(1900, 2, 29), "1900은 100의 배수라 평년");
        assert!(is_valid_date(2000, 2, 29), "2000은 400의 배수라 윤년");
        assert!(!is_valid_date(2023, 2, 29), "2023은 평년");
        assert!(!is_valid_date(2024, 4, 31), "4월은 30일까지");
        assert!(!is_valid_date(2024, 0, 1));
        assert!(!is_valid_date(2024, 13, 1));
    }

    #[test]
    fn to_digits_rejects_non_digits() {
        assert_eq!(to_digits("0123"), Some(vec![0, 1, 2, 3]));
        assert_eq!(to_digits("01-23"), None);
        assert_eq!(to_digits(""), Some(vec![]));
    }
}
