//! 2층: 탐지.
//!
//! 정규화문 위에서 후보를 찾고, 검증식과 문맥으로 걸러 [`Finding`] 을 낸다. 스팬은
//! [`crate::span::SpanMap`] 으로 원문 좌표에 되돌린 뒤 실린다.
//!
//! ```
//! use rust_pii_transformer::detect::{detect, Config, Certainty, EntityKind};
//!
//! let text = "주민등록번호 팔팔공일공일 - 1234567 입니다";
//! let report = detect(text, &Config::default()).unwrap();
//!
//! let finding = &report.findings[0];
//! assert_eq!(finding.entity, EntityKind::Resident);
//! // 한글 수사로 적힌 앞자리까지 원문 구간으로 되돌아온다.
//! assert_eq!(finding.source.slice(text), "팔팔공일공일 - 1234567");
//! assert_eq!(finding.evidence.cost.expanded_syllables, 6);
//! assert!(finding.certainty >= Certainty::Probable);
//! ```
//!
//! ## 점수 계산
//!
//! ```text
//! 점수 = 패턴 기본점
//!      + 검증식 통과 가산점
//!      + 문맥 단서 가산점 (단서 무게와 거리 감쇠 반영)
//!      - 정규화 비용 감점 (흡수 공백 > 흡수 구분자 > 확장 음절 > 치환 문자 순으로 무겁게)
//! ```
//!
//! 감점이 있는 이유는 "정규화는 관대하게, 검증은 엄격하게" 원칙의 대가를 계량화하기 위해서다.
//! 공백을 세 개 흡수해서 만들어진 숫자열은 원래부터 붙어 있던 숫자열보다 덜 믿을 만하다.

pub mod checksum;
pub mod context;
pub mod entity;
pub mod scanner;

use serde::Serialize;

pub use checksum::ChecksumResult;
pub use context::ContextHit;
pub use entity::{Candidate, Certainty, EntityKind};

use crate::error::Result;
use crate::normalize::{normalize, NormalizeConfig};
use crate::span::{NormalizationCost, RuleId, Span};

/// 점수 가중치. 모두 조정 가능하다.
#[derive(Debug, Clone, PartialEq)]
pub struct Weights {
    /// 검증식 통과 가산점.
    pub checksum_passed: f32,
    /// 검증식 실패 감점. 버리지 않고 깎기만 한다.
    pub checksum_failed: f32,
    /// 문맥 단서 총점에 곱하는 계수.
    pub context: f32,
    /// 흡수한 공백 한 개당 감점. 없던 숫자열을 만들 수 있어 가장 무겁다.
    pub absorbed_whitespace: f32,
    /// 흡수한 구분자 한 개당 감점.
    pub absorbed_separator: f32,
    /// 숫자로 펼친 한글 음절 한 개당 감점.
    pub expanded_syllable: f32,
    /// 치환한 문자 한 개당 감점. 전각 폴딩은 값을 바꾸지 않아 가장 가볍다.
    pub replaced_char: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            checksum_passed: 0.6,
            checksum_failed: 0.25,
            context: 0.5,
            absorbed_whitespace: 0.08,
            absorbed_separator: 0.01,
            expanded_syllable: 0.01,
            replaced_char: 0.002,
        }
    }
}

/// 탐지 설정.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 정규화 파이프라인 설정.
    pub normalize: NormalizeConfig,
    /// 문맥 단서를 찾을 창 크기(문자).
    pub context_window: u32,
    /// 이 점수 미만이면 결과로 내지 않는다.
    pub min_score: f32,
    /// 문맥이 필수인 엔티티가 요구하는 최소 문맥 총점.
    pub min_context: f32,
    /// 부정 문맥이 후보를 버릴 수 있게 되는 최소 총점.
    ///
    /// 이 값 미만이면 부정 단서가 있어도 아무 일도 하지 않는다. 0 으로 두면 스치듯 걸린
    /// 낱말 하나가 판정을 뒤집는다. 자세한 규칙은 [`context::EXCLUDE`] 에 적었다.
    pub min_veto: f32,
    /// 점수 가중치.
    pub weights: Weights,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            normalize: NormalizeConfig::default(),
            context_window: 24,
            min_score: 0.5,
            min_context: 0.3,
            min_veto: 0.5,
            weights: Weights::default(),
        }
    }
}

/// 왜 이 후보가 걸렸는가.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    /// 걸린 패턴 규칙 이름.
    pub rule: &'static str,
    /// 검증식 결과. 적용 불가면 그 사유까지 담는다.
    pub checksum: ChecksumResult,
    /// 잡힌 문맥 단서와 스팬으로부터의 거리.
    pub context_hits: Vec<ContextHit>,
    /// 이 스팬에 적용된 정규화 규칙 목록.
    pub normalizations: Vec<RuleId>,
    /// 정규화 비용. 감점 근거다.
    pub cost: NormalizationCost,
    /// 세그먼트 경계 바깥으로 넓혀졌는가.
    pub snapped: bool,
}

/// 탐지 결과 한 건.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// 무슨 개인정보인가.
    pub entity: EntityKind,
    /// 원문 기준 스팬. 마스킹은 이 구간에 적용된다.
    pub source: Span,
    /// 정규화문 기준 스팬.
    pub normalized: Span,
    /// 판정 등급.
    pub certainty: Certainty,
    /// 연속 점수.
    pub score: f32,
    /// 판정 근거.
    pub evidence: Evidence,
}

/// 왜 떨어졌는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// 검증식에서 떨어졌고 문맥도 부족했다.
    ChecksumFailed,
    /// 문맥이 필수인 엔티티인데 단서가 없었다.
    NoContext,
    /// 점수가 문턱에 못 미쳤다.
    BelowThreshold,
    /// 같은 구간에서 더 강한 후보에게 밀렸다. 더 넓은 결과에 삼켜진 경우도 여기다.
    Outranked,
    /// 주변 낱말이 개인정보가 아니라고 말했다. 계약번호·상품코드 따위다.
    BusinessContext,
}

/// 형식은 맞았는데 판정에서 떨어진 후보.
///
/// "검증되지 않은 개인정보가 지나갔다"는 의심을 조사할 때 필요하다.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rejection {
    /// 무슨 엔티티로 봤던 후보인가.
    pub entity: EntityKind,
    /// 원문 기준 스팬.
    pub source: Span,
    /// 떨어진 사유.
    pub reason: RejectReason,
    /// 그때의 점수.
    pub score: f32,
}

/// 탐지 결과 전체.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// 통과한 결과. 원문 위치 오름차순으로 정렬된다.
    pub findings: Vec<Finding>,
    /// 떨어진 후보. 왜 안 걸렸는지를 설명한다.
    pub rejections: Vec<Rejection>,
    /// 정규화된 텍스트. 판정 근거를 눈으로 확인할 때 쓴다.
    pub normalized_text: String,
}

/// 텍스트에서 개인정보를 탐지한다.
pub fn detect(text: &str, cfg: &Config) -> Result<Report> {
    let normalized = normalize(text, &cfg.normalize)?;
    let mut findings = Vec::new();
    let mut rejections = Vec::new();

    // 이메일은 숫자 런과 겹치지 않으므로 따로 모은다.
    for span in scanner::emails(&normalized.text) {
        let source = normalized.map.to_source(&span);
        findings.push(Finding {
            entity: EntityKind::Email,
            source: source.span,
            normalized: span,
            certainty: Certainty::Probable,
            score: 0.9,
            evidence: Evidence {
                rule: "email.structural",
                checksum: ChecksumResult::NotApplicable("이메일에는 검증식이 없다"),
                context_hits: Vec::new(),
                normalizations: source.rules,
                cost: source.cost,
                snapped: source.snapped,
            },
        });
    }

    // 여권번호는 영문자를 포함해 숫자 런에 잡히지 않는다. 별도로 훑는다.
    for span in scanner::passports(&normalized.text) {
        let source = normalized.map.to_source(&span);
        let candidate = entity::passport_candidate();
        let hits = context::find(&normalized.text, &span, cfg.context_window, candidate.cues);
        let context_total = context::total(&hits);
        let veto = veto_total(&normalized.text, &span, cfg);
        let evaluated = evaluate(&candidate, context_total, &source.cost, &cfg.weights);

        let reason = if vetoed(&candidate, context_total, veto, cfg) {
            Some(RejectReason::BusinessContext)
        } else if context_total < cfg.min_context {
            Some(RejectReason::NoContext)
        } else if evaluated.score < cfg.min_score {
            Some(RejectReason::BelowThreshold)
        } else {
            None
        };

        match reason {
            Some(reason) => rejections.push(Rejection {
                entity: candidate.entity,
                source: source.span,
                reason,
                score: evaluated.score,
            }),
            None => findings.push(Finding {
                entity: candidate.entity,
                source: source.span,
                normalized: span,
                certainty: evaluated.certainty,
                score: evaluated.score,
                evidence: Evidence {
                    rule: candidate.rule,
                    checksum: candidate.checksum,
                    context_hits: hits,
                    normalizations: source.rules,
                    cost: source.cost,
                    snapped: source.snapped,
                },
            }),
        }
    }

    for span in scanner::digit_runs(&normalized.text) {
        let digits = match checksum::to_digits(span.slice(&normalized.text)) {
            Some(digits) => digits,
            None => continue,
        };
        let source = normalized.map.to_source(&span);

        let mut best: Option<(Finding, f32)> = None;
        let mut local_rejections = Vec::new();
        let veto = veto_total(&normalized.text, &span, cfg);

        for candidate in entity::candidates(&digits) {
            let hits = context::find(&normalized.text, &span, cfg.context_window, candidate.cues);
            let context_total = context::total(&hits);

            let evaluated = evaluate(&candidate, context_total, &source.cost, &cfg.weights);

            let reason = if vetoed(&candidate, context_total, veto, cfg) {
                Some(RejectReason::BusinessContext)
            } else if candidate.needs_context && context_total < cfg.min_context {
                Some(RejectReason::NoContext)
            } else if evaluated.score < cfg.min_score {
                Some(if candidate.checksum.failed() {
                    RejectReason::ChecksumFailed
                } else {
                    RejectReason::BelowThreshold
                })
            } else {
                None
            };

            if let Some(reason) = reason {
                local_rejections.push(Rejection {
                    entity: candidate.entity,
                    source: source.span.clone(),
                    reason,
                    score: evaluated.score,
                });
                continue;
            }

            let finding = Finding {
                entity: candidate.entity,
                source: source.span.clone(),
                normalized: span.clone(),
                certainty: evaluated.certainty,
                score: evaluated.score,
                evidence: Evidence {
                    rule: candidate.rule,
                    checksum: candidate.checksum,
                    context_hits: hits,
                    normalizations: source.rules.clone(),
                    cost: source.cost,
                    snapped: source.snapped,
                },
            };

            let wins = best
                .as_ref()
                .map_or(true, |(_, best_score)| evaluated.score > *best_score);
            if wins {
                // 밀려난 이전 최고 후보도 왜 안 나왔는지 남긴다.
                if let Some((previous, score)) = best.replace((finding, evaluated.score)) {
                    local_rejections.push(Rejection {
                        entity: previous.entity,
                        source: previous.source,
                        reason: RejectReason::Outranked,
                        score,
                    });
                }
            } else {
                local_rejections.push(Rejection {
                    entity: candidate.entity,
                    source: source.span.clone(),
                    reason: RejectReason::Outranked,
                    score: evaluated.score,
                });
            }
        }

        if let Some((finding, _)) = best {
            findings.push(finding);
        }
        rejections.append(&mut local_rejections);
    }

    drop_contained(&mut findings, &mut rejections);

    findings.sort_by_key(|f| (f.source.byte.start, f.source.byte.end));
    rejections.sort_by_key(|r| (r.source.byte.start, r.source.byte.end));

    Ok(Report {
        findings,
        rejections,
        normalized_text: normalized.text,
    })
}

/// 더 넓은 결과에 통째로 삼켜진 결과를 걷어낸다.
///
/// 스캐너가 둘이라 같은 자리를 두 번 낼 수 있다. 여권번호 `M12345678` 안에는 여덟 자리 숫자
/// 런이 들어 있어 여권 스캐너와 숫자 스캐너가 각각 후보를 낸다. **넓은 쪽이 좁은 쪽을 이미
/// 설명하므로** 좁은 쪽을 버린다. 버린 것도 사유와 함께 남겨 조사할 수 있게 둔다.
///
/// 같은 폭이면 아무것도 버리지 않는다. 같은 구간 안에서의 경쟁은 후보 평가 단계가 이미 끝냈다.
fn drop_contained(findings: &mut Vec<Finding>, rejections: &mut Vec<Rejection>) {
    let spans: Vec<Span> = findings.iter().map(|f| f.source.clone()).collect();
    let swallowed: Vec<bool> = spans
        .iter()
        .map(|mine| {
            spans.iter().any(|other| {
                other.byte.start <= mine.byte.start
                    && mine.byte.end <= other.byte.end
                    && other.byte.end - other.byte.start > mine.byte.end - mine.byte.start
            })
        })
        .collect();

    let mut at = 0;
    findings.retain(|finding| {
        let keep = !swallowed[at];
        if !keep {
            rejections.push(Rejection {
                entity: finding.entity,
                source: finding.source.clone(),
                reason: RejectReason::Outranked,
                score: finding.score,
            });
        }
        at += 1;
        keep
    });
}

/// 이 구간 주변의 부정 문맥 총점.
fn veto_total(text: &str, span: &Span, cfg: &Config) -> f32 {
    context::total(&context::find(
        text,
        span,
        cfg.context_window,
        context::EXCLUDE,
    ))
}

/// 부정 문맥이 이 후보를 버리는가.
///
/// 조건이 셋 다 서야 버린다. **검증식을 통과하지 않았고**, 부정 총점이 문턱을 넘었고,
/// 그 총점이 긍정 총점보다 무거울 때다. 근거를 세는 방식은 [`context::EXCLUDE`] 에 적었다.
fn vetoed(candidate: &Candidate, context_total: f32, veto: f32, cfg: &Config) -> bool {
    !candidate.checksum.passed() && veto >= cfg.min_veto && veto > context_total
}

struct Evaluated {
    score: f32,
    certainty: Certainty,
}

/// 후보 하나의 점수와 등급을 낸다.
fn evaluate(
    candidate: &Candidate,
    context_total: f32,
    cost: &NormalizationCost,
    weights: &Weights,
) -> Evaluated {
    let mut score = candidate.base;

    if candidate.checksum.passed() {
        score += weights.checksum_passed;
    } else if candidate.checksum.failed() {
        score -= weights.checksum_failed;
    }
    score += context_total * weights.context;

    score -= f32::from(cost.absorbed_whitespace) * weights.absorbed_whitespace;
    score -= f32::from(cost.absorbed_separators) * weights.absorbed_separator;
    score -= f32::from(cost.expanded_syllables) * weights.expanded_syllable;
    score -= f32::from(cost.replaced_chars) * weights.replaced_char;

    let certainty = if candidate.checksum.passed() {
        Certainty::Certain
    } else if context_total > 0.0 {
        Certainty::Probable
    } else {
        Certainty::Possible
    };

    Evaluated {
        score: score.clamp(0.0, 2.0),
        // 검증식이 없거나 공개되지 않은 엔티티는 천장을 넘을 수 없다.
        certainty: certainty.min(candidate.ceiling),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Report {
        detect(text, &Config::default()).unwrap()
    }

    fn entities(report: &Report) -> Vec<EntityKind> {
        report.findings.iter().map(|f| f.entity).collect()
    }

    #[test]
    fn finds_a_resident_number_written_in_hangul_numerals() {
        let text = "주민등록번호 팔팔공일공일 - 1234567 입니다";
        let report = run(text);
        assert_eq!(entities(&report), vec![EntityKind::Resident]);

        let finding = &report.findings[0];
        assert_eq!(finding.source.slice(text), "팔팔공일공일 - 1234567");
        assert_eq!(finding.evidence.cost.expanded_syllables, 6);
        assert_eq!(finding.evidence.cost.absorbed_whitespace, 2);
        assert!(finding.evidence.context_hits.iter().any(|h| h.cue == "주민등록번호"));
    }

    #[test]
    fn finds_a_card_number_without_any_context() {
        // Luhn 은 국제 표준이라 문맥이 없어도 확실하다.
        let report = run("4111-1111-1111-1111");
        assert_eq!(entities(&report), vec![EntityKind::CreditCard]);
        assert_eq!(report.findings[0].certainty, Certainty::Certain);
    }

    #[test]
    fn finds_a_business_number() {
        let report = run("사업자등록번호 124-81-00998");
        assert_eq!(entities(&report), vec![EntityKind::BusinessRegistration]);
        assert_eq!(report.findings[0].certainty, Certainty::Certain);
    }

    #[test]
    fn finds_a_phone_number() {
        let report = run("연락처는 010-1234-5678 입니다");
        assert_eq!(entities(&report), vec![EntityKind::Phone]);
        assert_eq!(
            report.findings[0].certainty,
            Certainty::Probable,
            "전화번호는 검증식이 없어 천장이 Probable 이다"
        );
    }

    /// 회귀 테스트. 문맥 없는 하이픈·전각 전화번호가 문턱에서 떨어지던 결함을 막는다.
    ///
    /// 기본점이 문턱과 같으면 정규화 비용이 판정을 뒤집는다. 합성 코퍼스 실측에서 전화번호
    /// 재현율 66.7퍼센트로 드러났고, 미탐이 전부 이 두 표기였다.
    #[test]
    fn a_bare_phone_number_survives_hyphen_and_fullwidth_cost() {
        for text in ["010-1234-5678", "01012345678", "０１０１２３４５６７８"] {
            let report = run(text);
            assert_eq!(
                entities(&report),
                vec![EntityKind::Phone],
                "문맥 없는 전화번호를 놓쳤다: {text}"
            );
        }
    }

    /// 공백으로 끊은 전화번호는 문맥이 있어야 잡힌다. **결함이 아니라 설계된 경계다.**
    ///
    /// 공백 흡수는 감점이 가장 무겁다(하이픈의 여덟 배). 없던 숫자열을 만들어 내는 유일한
    /// 정규화이기 때문이다. 문맥 없이 공백 표기까지 잡으려면 그 장치를 풀어야 하고, 그러면
    /// 세 덩어리로 떨어져 있던 숫자가 우연히 식별번호로 시작하는 순간 전부 전화번호가 된다.
    /// 여기서는 재현율보다 정밀도를 택했다.
    #[test]
    fn a_space_separated_phone_number_needs_context() {
        assert!(
            entities(&run("010 1234 5678")).is_empty(),
            "문맥 없는 공백 표기는 의도적으로 버린다"
        );
        assert_eq!(
            entities(&run("연락처는 010 1234 5678 입니다")),
            vec![EntityKind::Phone],
            "문맥이 있으면 공백 표기도 잡아야 한다"
        );
    }

    #[test]
    fn finds_an_email() {
        let report = run("메일은 minsu.kim@example.com 입니다");
        assert_eq!(entities(&report), vec![EntityKind::Email]);
    }

    #[test]
    fn finds_a_foreigner_registration_number() {
        let report = run("외국인등록번호 880101-5234567");
        assert_eq!(entities(&report), vec![EntityKind::ForeignerRegistration]);
    }

    #[test]
    fn account_number_needs_context() {
        // 문맥이 없으면 계좌번호로 보지 않는다. 이 라이브러리의 오탐 억제 핵심이다.
        let bare = run("첨부 자료 1234567890123 참고하세요");
        assert!(!entities(&bare).contains(&EntityKind::BankAccount));
        assert!(bare
            .rejections
            .iter()
            .any(|r| r.entity == EntityKind::BankAccount && r.reason == RejectReason::NoContext));

        let with_context = run("입금 계좌번호 1234567890123 로 보내주세요");
        assert!(entities(&with_context).contains(&EntityKind::BankAccount));
    }

    #[test]
    fn resident_number_with_a_valid_checksum_reaches_certain() {
        // 880101-1234568 은 검증식을 만족한다(계산으로 확인한 값이다).
        let report = run("주민등록번호 880101-1234568 입니다");
        let finding = &report.findings[0];
        assert_eq!(finding.entity, EntityKind::Resident);
        assert!(finding.evidence.checksum.passed());
        assert_eq!(finding.certainty, Certainty::Certain);
    }

    #[test]
    fn resident_number_survives_a_failed_checksum_when_context_is_strong() {
        // 2020년 10월 이후 발급분은 검증식이 성립하지 않는다. 검증 실패를 버리는 근거가 아니라
        // 등급을 낮추는 근거로만 쓴다. 이것이 이 라이브러리의 핵심 판단 중 하나다.
        let report = run("주민등록번호 880101-1234567 입니다");
        let finding = report
            .findings
            .iter()
            .find(|f| f.entity == EntityKind::Resident)
            .expect("검증식이 떨어져도 문맥이 있으면 살아야 한다");
        assert!(finding.evidence.checksum.failed());
        assert_eq!(finding.certainty, Certainty::Probable);
    }

    // ── 마스킹된 값 ────────────────────────────────────────

    /// 이미 부분 마스킹된 문서를 다시 처리하는 경우. 숫자가 아니라는 이유로 버리면 그대로 샌다.
    #[test]
    fn a_masked_resident_number_is_detected() {
        let text = "주민등록번호 900513-1****67 입니다";
        let report = run(text);
        let finding = report
            .findings
            .iter()
            .find(|f| f.entity == EntityKind::Resident)
            .expect("가려진 자리가 있어도 주민등록번호로 잡혀야 한다");
        assert_eq!(finding.source.slice(text), "900513-1****67");
    }

    /// 모른다는 사실이 등급으로 표현되어야 한다.
    #[test]
    fn a_masked_value_cannot_reach_certain() {
        let report = run("주민등록번호 900513-1****67 입니다");
        let finding = &report.findings[0];
        assert!(
            matches!(finding.evidence.checksum, ChecksumResult::NotApplicable(_)),
            "가려진 자리가 있으면 검증식을 적용할 수 없다"
        );
        assert!(finding.certainty < Certainty::Certain);
    }

    #[test]
    fn a_masked_card_number_survives_separator_absorption() {
        let text = "카드번호 1234-****-****-5678 로 결제";
        let report = run(text);
        let finding = report
            .findings
            .iter()
            .find(|f| f.entity == EntityKind::CreditCard)
            .expect("마스킹된 카드번호도 한 덩어리로 모여야 한다");
        assert_eq!(finding.source.slice(text), "1234-****-****-5678");
    }

    /// 마스킹 문자를 런에 넣으면 잡음이 늘 수 있다. 실제 숫자 두 자리 문턱이 그것을 막는다.
    #[test]
    fn mask_characters_alone_are_not_candidates() {
        for text in [
            "**굵게** 표시한 문장",
            "가로 세로 비율은 3x4 입니다",
            "X 표시를 눌러 닫습니다",
        ] {
            let report = run(text);
            assert!(report.findings.is_empty(), "잡음이 후보가 됐다: {text}");
        }
    }

    /// 앞자리가 가려지면 날짜 제약이 사라진다. 그 자리를 문맥이 메운다.
    #[test]
    fn a_fully_masked_prefix_requires_context() {
        assert!(
            entities(&run("접수 ****131234567 참고")).is_empty(),
            "문맥이 없으면 날짜를 못 확인한 13자리를 내지 않는다"
        );
        assert!(
            entities(&run("주민등록번호 ****131234567 입니다")).contains(&EntityKind::Resident),
            "문맥이 있으면 남긴다"
        );
    }

    #[test]
    fn a_passport_number_needs_context() {
        assert_eq!(
            entities(&run("여권번호 M12345678 로 예약했습니다")),
            vec![EntityKind::Passport]
        );
        assert!(
            entities(&run("모델 M12345678 재고 확인 바랍니다")).is_empty(),
            "여권 단서가 없으면 제품 코드와 구분되지 않는다"
        );
    }

    /// 여권번호는 검증식이 없어 문맥이 아무리 좋아도 확정 등급에 오르지 않는다.
    #[test]
    fn a_passport_number_cannot_reach_certain() {
        let report = run("여권번호 M12345678 입니다");
        assert_eq!(report.findings[0].certainty, Certainty::Probable);
    }

    /// 여권번호 안의 숫자 런이 계좌번호 따위로 따로 잡히면 안 된다.
    #[test]
    fn the_digit_run_inside_a_passport_is_swallowed() {
        let report = run("여권번호 M12345678 로 예약했습니다");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].source.slice("여권번호 M12345678 로 예약했습니다"), "M12345678");
    }

    #[test]
    fn business_words_drop_candidates_that_have_no_checksum() {
        let report = run("접수번호 1234567890 로 조회하세요");
        assert!(entities(&report).is_empty());
        assert!(
            report
                .rejections
                .iter()
                .any(|r| r.reason == RejectReason::BusinessContext),
            "왜 버렸는지가 사유로 남아야 한다"
        );
    }

    /// 검증식을 통과한 값은 이름표가 잘못 붙어 있어도 개인정보다.
    #[test]
    fn a_valid_checksum_survives_business_words() {
        // 880101-1234568 은 검증식을 만족한다.
        let report = run("증권번호 880101-1234568 조회");
        assert!(entities(&report).contains(&EntityKind::Resident));
    }

    /// 두 종류의 낱말이 한 창에 같이 들어오는 문장이 실무에 흔하다.
    #[test]
    fn a_nearer_positive_cue_outweighs_a_business_word() {
        let report = run("계약번호 확인 후 연락처 010-1234-5678 로 회신 바랍니다");
        assert!(entities(&report).contains(&EntityKind::Phone));
    }

    /// 회사 안내 번호까지 개인정보로 세면 마스킹 결과가 읽히지 않는다.
    #[test]
    fn a_call_center_number_is_not_personal() {
        assert!(entities(&run("고객센터 1588-1234 로 문의하세요")).is_empty());
    }

    /// 부정 문맥을 끄는 손잡이가 실제로 듣는지 본다.
    #[test]
    fn raising_min_veto_disarms_the_business_dictionary() {
        let text = "접수번호 010-1234-5678 로 조회하세요";
        assert!(entities(&run(text)).is_empty());

        let cfg = Config { min_veto: 99.0, ..Config::default() };
        let report = detect(text, &cfg).unwrap();
        assert!(report
            .findings
            .iter()
            .any(|f| f.entity == EntityKind::Phone));
    }

    #[test]
    fn plain_text_yields_nothing() {
        let report = run("이번 분기 실적은 전년 대비 개선되었습니다. 자세한 내용은 보고서를 참고하세요.");
        assert!(report.findings.is_empty());
        assert!(report.rejections.is_empty());
    }

    #[test]
    fn order_numbers_are_not_flagged() {
        for text in [
            "주문번호 20240115003 로 접수되었습니다",
            "제품 코드 1234 수량 5678 개",
            "송장번호 987654321098 입니다",
        ] {
            let report = run(text);
            assert!(
                report.findings.is_empty(),
                "오탐이 났다: {text} -> {:?}",
                entities(&report)
            );
        }
    }

    #[test]
    fn rejections_explain_why_a_candidate_was_dropped() {
        let report = run("접수번호 1234567890123 입니다");
        assert!(!report.rejections.is_empty(), "떨어진 이유가 남아야 한다");
        for rejection in &report.rejections {
            assert!(rejection.score >= 0.0);
        }
    }

    #[test]
    fn findings_are_sorted_by_position() {
        let text = "카드 4111-1111-1111-1111 이고 연락처 010-1234-5678 이며 메일 a@b.com";
        let report = run(text);
        for pair in report.findings.windows(2) {
            assert!(pair[0].source.byte.start <= pair[1].source.byte.start);
        }
        assert_eq!(report.findings.len(), 3);
    }

    #[test]
    fn fullwidth_digits_are_detected() {
        let report = run("카드번호 ４１１１１１１１１１１１１１１１");
        assert_eq!(entities(&report), vec![EntityKind::CreditCard]);
        assert!(report.findings[0].evidence.cost.replaced_chars > 0);
    }

    #[test]
    fn one_span_yields_at_most_one_finding() {
        // 열세 자리는 주민등록번호이자 카드번호 후보다. 하나만 남아야 한다.
        let report = run("주민등록번호 8801011234567");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].entity, EntityKind::Resident);
    }

    /// 회귀 테스트. 같은 값에 이름이 둘 생기던 결함을 막는다.
    ///
    /// Python 바인딩과 명령줄 도구는 엔티티를 `"resident"` 로 부르는데 JSON 직렬화는
    /// `"Resident"` 를 내고 있었다. 어느 문으로 들어오느냐에 따라 이름이 달라지면
    /// 소비자가 두 이름을 다 알아야 한다.
    #[test]
    fn json_names_match_the_names_the_api_reports() {
        let report = run("주민등록번호 880101-1234568 이고 연락처 010-1234-5678");
        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains(r#""entity":"resident""#), "{json}");
        assert!(json.contains(r#""entity":"phone""#), "{json}");
        assert!(json.contains(r#""certainty":"certain""#), "{json}");
        assert!(json.contains(r#""checksum":"passed""#), "{json}");
        assert!(
            !json.contains(r#""Resident""#) && !json.contains(r#""Certain""#),
            "파스칼 케이스 이름이 남아 있다: {json}"
        );
    }

    #[test]
    fn thresholds_are_configurable() {
        let strict = Config {
            min_score: 1.5,
            ..Default::default()
        };
        let report = detect("연락처는 010-1234-5678 입니다", &strict).unwrap();
        assert!(report.findings.is_empty(), "문턱을 올리면 걸러진다");
    }
}
