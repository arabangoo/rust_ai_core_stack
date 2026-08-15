//! 1층: 정규화 파이프라인.
//!
//! 원문을 탐지하기 좋은 형태로 펴면서, 동시에 원문으로 되돌아가는 길([`SpanMap`])을 만든다.
//! 패스는 순서대로 적용되고 각 패스는 자기 매핑을 낸다. 매핑은 [`SpanMap::compose`] 로
//! 합쳐지므로 패스를 늘리거나 빼도 복원 코드는 바뀌지 않는다.
//!
//! | 순서 | 패스 | 하는 일 |
//! | --- | --- | --- |
//! | 1 | [`nfc`] | 자모 분리형 한글을 음절로 합친다 |
//! | 2 | [`fold`] | 전각을 반각으로, 대시와 공백 변종을 통일한다 |
//! | 3 | [`hangul`] | 한글 수사를 숫자로 되돌린다 |
//! | 4 | [`lookalike`] | 숫자 사이에 낀 유사문자를 숫자로 되돌린다 |
//! | 5 | [`separator`] | 숫자 사이의 붙임표, 점, 공백, 괄호를 흡수한다 |
//!
//! 순서에는 이유가 있다. 자모를 먼저 합쳐야 수사 음절이 음절 단위로 보이고, 전각을 먼저
//! 접어야 전각 숫자가 숫자로 보이며, 수사를 먼저 숫자로 바꿔야 구분자 흡수의 "양쪽이 숫자"
//! 조건이 성립한다. [`lookalike`] 가 [`separator`] 앞인 것도 같은 이유의 반대편이다.
//! 구분자가 아직 남아 있어야 `88-O1-O1` 의 `O` 가 양옆에서 숫자를 볼 수 있다.
//!
//! ```
//! use rust_pii_transformer::normalize::{normalize, NormalizeConfig};
//!
//! let out = normalize("팔팔공일공일 - 1234567", &NormalizeConfig::default()).unwrap();
//! assert_eq!(out.text, "8801011234567");
//!
//! // 정규화문 전체를 원문 좌표로 되돌린다.
//! // 한글 6음절 18바이트 + 구분자 3바이트 + 숫자 7바이트 = 28바이트.
//! let src = out.map.to_source(&rust_pii_transformer::Span::new(0..13, 0..13));
//! assert_eq!(src.span.byte, 0..28);
//! assert_eq!(src.cost.expanded_syllables, 6);
//! assert_eq!(src.cost.absorbed_whitespace, 2);
//! ```

pub mod fold;
pub mod hangul;
pub mod lookalike;
pub mod nfc;
pub mod separator;

pub use hangul::NumeralConfig;

use crate::error::Result;
use crate::span::SpanMap;

/// 정규화 결과. 탐지 층은 [`Normalized::text`] 위에서 스캔하고,
/// 찾은 스팬을 [`Normalized::map`] 으로 원문 좌표에 되돌린다.
#[derive(Debug, Clone)]
pub struct Normalized {
    /// 정규화된 텍스트.
    pub text: String,
    /// 원문과 정규화문을 잇는 단조 구간 정렬 테이블.
    pub map: SpanMap,
}

/// 정규화 파이프라인 설정. 패스를 개별로 끌 수 있다.
///
/// 끄는 것이 유용한 경우가 실제로 있다. 소수점이 많은 회계 문서에서는 [`separator`] 패스가
/// `1.5` 를 `15` 로 만들고, 그것이 오탐의 출처가 된다. 그럴 때 그 패스만 꺼서 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizeConfig {
    /// 한글 자모 조합 패스를 켠다.
    pub nfc: bool,
    /// 전각 폴딩 패스를 켠다.
    pub fold: bool,
    /// 한글 수사 역변환 패스를 켠다.
    pub hangul: bool,
    /// 숫자 사이 유사문자 교정 패스를 켠다.
    pub lookalike: bool,
    /// 구분자 흡수 패스를 켠다.
    pub separator: bool,
    /// 한글 수사 역변환의 오탐 억제 설정.
    pub numeral: NumeralConfig,
}

impl Default for NormalizeConfig {
    fn default() -> Self {
        Self {
            nfc: true,
            fold: true,
            hangul: true,
            lookalike: true,
            separator: true,
            numeral: NumeralConfig::default(),
        }
    }
}

impl NormalizeConfig {
    /// 문맥에 의존하지 않는 패스만 켠 설정.
    ///
    /// [`nfc`] 와 [`fold`] 는 한 문자를 보는 것만으로 변환이 결정되므로, 텍스트를 잘라 낸
    /// 조각을 따로 정규화해도 결과가 같다. 왕복 불변식을 조각 단위로 검증할 수 있는 것도
    /// 이 두 패스뿐이다. 자세한 이유는 이 모듈의 테스트 주석에 적었다.
    pub fn context_free() -> Self {
        Self {
            nfc: true,
            fold: true,
            hangul: false,
            lookalike: false,
            separator: false,
            numeral: NumeralConfig::default(),
        }
    }
}

/// 정규화 파이프라인을 적용한다.
///
/// 에러는 [`SpanMap::compose`] 의 좌표계 불일치뿐이고, 그것은 패스 구현 버그를 뜻한다.
/// 사용자 입력으로는 발생하지 않는다.
pub fn normalize(text: &str, cfg: &NormalizeConfig) -> Result<Normalized> {
    let mut current = text.to_string();
    let mut map = SpanMap::identity(text);

    if cfg.nfc {
        let (out, pass) = nfc::apply(&current);
        map = SpanMap::compose(&map, &pass)?;
        current = out;
    }
    if cfg.fold {
        let (out, pass) = fold::apply(&current);
        map = SpanMap::compose(&map, &pass)?;
        current = out;
    }
    if cfg.hangul {
        let (out, pass) = hangul::apply(&current, &cfg.numeral);
        map = SpanMap::compose(&map, &pass)?;
        current = out;
    }
    if cfg.lookalike {
        let (out, pass) = lookalike::apply(&current);
        map = SpanMap::compose(&map, &pass)?;
        current = out;
    }
    if cfg.separator {
        let (out, pass) = separator::apply(&current);
        map = SpanMap::compose(&map, &pass)?;
        current = out;
    }

    Ok(Normalized { text: current, map })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    /// 회귀 검증에 쓰는 표본. 각 패스와 그 조합을 골고루 건드린다.
    const CORPUS: &[&str] = &[
        "",
        "평범한 문장입니다.",
        "880101-1234567",
        "880101 - 1234567",
        "팔팔공일공일 일이삼사오육칠",
        "８８０１０１－１２３４５６７",
        "주민등록번호 팔팔공일공일 - 1234567 입니다",
        "구십일년 삼월 이십사일",
        "\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF} 880101",
        "제품 1234 수량 5678",
        "이사 갑니다. 사구려 구경하세요",
        "010.1234.5678 로 연락 주세요",
        "카드 4111-1111-1111-1111",
        "천구백팔십팔년생",
        "만원만 빌려줘",
        "전각\u{3000}공백\u{2013}대시",
    ];

    fn slice(text: &str, span: &Span) -> String {
        text[span.byte.start as usize..span.byte.end as usize].to_string()
    }

    /// 피복 불변식의 텍스트판. 세그먼트를 순서대로 이어 붙이면 양쪽 원본이 정확히 복원된다.
    ///
    /// `validate()` 는 오프셋 수준의 빈틈과 겹침을 보지만, 그 오프셋이 실제 텍스트를 빠짐없이
    /// 덮는지는 보지 않는다. 이 테스트가 그 마지막 한 칸을 채운다.
    #[test]
    fn segments_tile_both_texts() {
        for source in CORPUS {
            let out = normalize(source, &NormalizeConfig::default()).unwrap();
            out.map.validate().unwrap();

            let mut src_joined = String::new();
            let mut dst_joined = String::new();
            for segment in out.map.segments() {
                src_joined.push_str(&slice(source, &segment.src));
                dst_joined.push_str(&slice(&out.text, &segment.dst));
            }
            assert_eq!(&src_joined, source, "원문 복원 실패: {source:?}");
            assert_eq!(dst_joined, out.text, "정규화문 복원 실패: {source:?}");
        }
    }

    /// 정규화문 전체를 되돌리면 원문 전체가 나온다.
    #[test]
    fn full_span_round_trips() {
        for source in CORPUS {
            let out = normalize(source, &NormalizeConfig::default()).unwrap();
            let whole = Span::new(
                0..out.text.len() as u32,
                0..out.text.chars().count() as u32,
            );
            let recovered = out.map.to_source(&whole);
            assert_eq!(slice(source, &recovered.span), *source, "전체 왕복 실패: {source:?}");
        }
    }

    /// 왕복 불변식. 문맥 비의존 패스에 한정한다.
    ///
    /// 임의의 정규화 스팬을 원문으로 되돌린 뒤 그 조각을 **따로** 정규화하면 정규화문의 같은
    /// 구간이 나와야 한다. 이 성질은 [`NormalizeConfig::context_free`] 조합에서만 성립한다.
    ///
    /// 나머지 두 패스가 문맥을 보기 때문이다. `구십일년` 의 `구십일` 은 뒤의 `년` 이 있어야
    /// 숫자로 바뀌고, `880101 - 1234567` 의 구분자는 양옆이 숫자여야 흡수된다. 조각으로
    /// 잘라 내면 그 문맥이 사라지므로 같은 결과가 나오지 않는다. 이것은 결함이 아니라
    /// 오탐 억제 설계의 필연적 귀결이고, 전체 파이프라인의 복원 보장은 위 두 테스트
    /// (피복과 전체 왕복)가 대신 강제한다.
    #[test]
    fn round_trip_holds_for_context_free_passes() {
        let cfg = NormalizeConfig::context_free();
        for source in CORPUS {
            let out = normalize(source, &cfg).unwrap();
            let chars: Vec<(usize, char)> = out.text.char_indices().collect();

            for i in 0..chars.len() {
                for j in i + 1..=chars.len() {
                    let byte_start = chars[i].0 as u32;
                    let byte_end = chars.get(j).map_or(out.text.len(), |&(b, _)| b) as u32;
                    let span = Span::new(byte_start..byte_end, i as u32..j as u32);

                    let recovered = out.map.to_source(&span);
                    let fragment = slice(source, &recovered.span);
                    let renormalized = normalize(&fragment, &cfg).unwrap();

                    let expected = if recovered.snapped {
                        // 스냅이 붙었으면 넓어진 원문 구간에 대응하는 정규화 구간을 봐야 한다.
                        // 넓어진 쪽은 세그먼트 경계이므로 재정규화 결과가 그 구간과 일치한다.
                        renormalized.text.clone()
                    } else {
                        out.text[span.byte.start as usize..span.byte.end as usize].to_string()
                    };
                    assert_eq!(
                        renormalized.text, expected,
                        "왕복 실패: 원문 {source:?} 조각 {fragment:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn pipeline_order_lets_passes_feed_each_other() {
        // 전각 수사가 아니라 전각 숫자와 한글 수사가 섞인 입력에서 네 패스가 모두 걸린다.
        let out = normalize(
            "\u{1112}\u{1161}\u{11AB}국인 팔팔공일공일 － １２３４５６７",
            &NormalizeConfig::default(),
        )
        .unwrap();
        assert_eq!(out.text, "한국인 8801011234567");
    }

    #[test]
    fn disabled_pass_is_skipped() {
        let cfg = NormalizeConfig {
            separator: false,
            ..Default::default()
        };
        let out = normalize("880101 - 1234567", &cfg).unwrap();
        assert_eq!(out.text, "880101 - 1234567");
    }

    #[test]
    fn empty_input_is_handled() {
        let out = normalize("", &NormalizeConfig::default()).unwrap();
        assert_eq!(out.text, "");
        out.map.validate().unwrap();
    }

    #[test]
    fn evidence_accumulates_across_passes() {
        let out = normalize("팔팔공일공일 - 1234567", &NormalizeConfig::default()).unwrap();
        assert_eq!(out.text, "8801011234567");

        let whole = Span::new(0..13, 0..13);
        let src = out.map.to_source(&whole);
        assert_eq!(src.cost.expanded_syllables, 6, "한글 수사 6음절");
        assert_eq!(src.cost.absorbed_whitespace, 2, "공백 두 개");
        assert_eq!(src.cost.absorbed_separators, 1, "붙임표 한 개");
        assert!(src.rules.contains(&hangul::RULE_DIGIT));
        assert!(src.rules.contains(&separator::RULE_HYPHEN));
        assert!(src.rules.contains(&separator::RULE_WHITESPACE));
        // 규칙 목록은 적용 순서가 아니라 이름 오름차순으로 정규화된다.
        let mut sorted = src.rules.clone();
        sorted.sort_unstable();
        assert_eq!(src.rules, sorted);
    }
}
