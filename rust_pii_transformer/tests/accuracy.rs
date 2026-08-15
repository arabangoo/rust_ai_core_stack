//! 합성 코퍼스로 재현율·정밀도와 복원 왕복을 실측한다.
//!
//! 이 파일이 이 라이브러리의 완료 기준을 재는 장치다. "잘 된다"는 완료가 아니고,
//! 여기서 나오는 숫자가 완료의 근거다.
//!
//! 수치를 눈으로 보려면 이렇게 돌린다.
//!
//! ```bash
//! cargo test --test accuracy -- --nocapture
//! ```

use std::collections::BTreeMap;

use rust_pii_transformer::detect::{detect, Config, EntityKind, Finding};
use rust_pii_transformer::mask::{mask_report, unmask, Policy, PolicySet};
use rust_pii_transformer::synth::{corpus, Sample, Variant};

/// 코퍼스 규모. 씨앗과 함께 고정해 결과가 항상 재현되게 한다.
const SEED: u64 = 20260812;
const ROUNDS: usize = 40;

#[derive(Default, Clone, Copy)]
struct Tally {
    true_positive: u32,
    false_positive: u32,
    false_negative: u32,
}

impl Tally {
    fn recall(&self) -> f64 {
        let denom = self.true_positive + self.false_negative;
        if denom == 0 {
            return 1.0;
        }
        f64::from(self.true_positive) / f64::from(denom)
    }

    fn precision(&self) -> f64 {
        let denom = self.true_positive + self.false_positive;
        if denom == 0 {
            return 1.0;
        }
        f64::from(self.true_positive) / f64::from(denom)
    }

    fn add(&mut self, other: &Tally) {
        self.true_positive += other.true_positive;
        self.false_positive += other.false_positive;
        self.false_negative += other.false_negative;
    }
}

fn overlaps(finding: &Finding, sample: &Sample) -> bool {
    finding.source.byte.start < sample.span.end && sample.span.start < finding.source.byte.end
}

/// 표본 하나를 채점한다.
fn score(sample: &Sample, findings: &[Finding]) -> Tally {
    let mut tally = Tally::default();

    match sample.expected {
        Some(expected) => {
            let hit = findings.iter().any(|f| f.entity == expected && overlaps(f, sample));
            if hit {
                tally.true_positive += 1;
            } else {
                tally.false_negative += 1;
            }
            // 정답 말고 더 걸린 것은 오탐이다.
            tally.false_positive += findings
                .iter()
                .filter(|f| !(f.entity == expected && overlaps(f, sample)))
                .count() as u32;
        }
        None => {
            tally.false_positive += findings.len() as u32;
        }
    }

    tally
}

fn label(entity: EntityKind) -> &'static str {
    entity.label()
}

#[test]
fn measures_recall_and_precision_on_the_synthetic_corpus() {
    let samples = corpus(SEED, ROUNDS);
    let cfg = Config::default();

    let mut overall = Tally::default();
    let mut by_entity: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut by_variant: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut negatives = Tally::default();
    let mut negative_count = 0u32;
    let mut misses: Vec<String> = Vec::new();

    for sample in &samples {
        let report = detect(&sample.text, &cfg).expect("탐지는 실패하지 않는다");
        let tally = score(sample, &report.findings);

        overall.add(&tally);
        match sample.expected {
            Some(entity) => {
                by_entity.entry(label(entity)).or_default().add(&tally);
                by_variant.entry(sample.variant.label()).or_default().add(&tally);
                if tally.false_negative > 0 && misses.len() < 12 {
                    misses.push(format!(
                        "  미탐 [{}/{}{}] {}",
                        label(entity),
                        sample.variant.label(),
                        if sample.with_context { "" } else { "/무문맥" },
                        sample.text
                    ));
                }
            }
            None => {
                negatives.add(&tally);
                negative_count += 1;
                if tally.false_positive > 0 && misses.len() < 24 {
                    misses.push(format!(
                        "  오탐 [{}] {}",
                        sample.trap.unwrap_or("?"),
                        sample.text
                    ));
                }
            }
        }
    }

    let positives = samples.iter().filter(|s| s.expected.is_some()).count();

    println!("\n=== 합성 코퍼스 실측 (씨앗 {SEED}, {ROUNDS} 라운드) ===");
    println!("표본: 양성 {positives} 건, 음성 {negative_count} 건, 합계 {} 건", samples.len());
    println!("\n전체");
    println!(
        "  재현율 {:.1}%   정밀도 {:.1}%   (TP {} / FP {} / FN {})",
        overall.recall() * 100.0,
        overall.precision() * 100.0,
        overall.true_positive,
        overall.false_positive,
        overall.false_negative
    );

    println!("\n엔티티별");
    println!("  {:<18} {:>8} {:>8} {:>6} {:>6} {:>6}", "엔티티", "재현율", "정밀도", "TP", "FP", "FN");
    for (name, tally) in &by_entity {
        println!(
            "  {:<18} {:>7.1}% {:>7.1}% {:>6} {:>6} {:>6}",
            name,
            tally.recall() * 100.0,
            tally.precision() * 100.0,
            tally.true_positive,
            tally.false_positive,
            tally.false_negative
        );
    }

    println!("\n표기 변형별 재현율");
    println!("  {:<12} {:>8} {:>6} {:>6}", "표기", "재현율", "TP", "FN");
    for (name, tally) in &by_variant {
        println!(
            "  {:<12} {:>7.1}% {:>6} {:>6}",
            name,
            tally.recall() * 100.0,
            tally.true_positive,
            tally.false_negative
        );
    }

    println!("\n음성 표본 (개인정보 없음)");
    println!(
        "  오탐 {} 건 / {} 건 = {:.1}%",
        negatives.false_positive,
        negative_count,
        f64::from(negatives.false_positive) / f64::from(negative_count.max(1)) * 100.0
    );

    if !misses.is_empty() {
        println!("\n놓친 표본 표본추출");
        for line in &misses {
            println!("{line}");
        }
    }
    println!();

    // 회귀 방지 바닥선. 실측값 바로 아래에 두어 정상 변동은 통과시키고 퇴행만 잡는다.
    //
    // **엔티티별 바닥선이 전체 바닥선보다 중요하다.** 전화번호 재현율이 66.7퍼센트였을 때도
    // 전체는 95.6퍼센트여서 느슨한 전체 바닥선을 통과했다. 한 엔티티가 무너져도 나머지가
    // 가려 주기 때문이다. 그래서 축을 셋으로 나눈다.
    assert!(
        overall.recall() >= 0.99,
        "전체 재현율이 {:.3} 으로 바닥선 0.99 아래다",
        overall.recall()
    );
    assert!(
        overall.precision() >= 0.98,
        "전체 정밀도가 {:.3} 으로 바닥선 0.98 아래다",
        overall.precision()
    );
    for (name, tally) in &by_entity {
        assert!(
            tally.recall() >= 0.93,
            "엔티티 '{name}' 의 재현율이 {:.3} 으로 바닥선 0.93 아래다",
            tally.recall()
        );
    }
    for (name, tally) in &by_variant {
        assert!(
            tally.recall() >= 0.97,
            "표기 변형 '{name}' 의 재현율이 {:.3} 으로 바닥선 0.97 아래다",
            tally.recall()
        );
    }
}

#[test]
fn masking_round_trips_on_every_corpus_sample() {
    let samples = corpus(SEED, ROUNDS);
    let cfg = Config::default();
    let policies = PolicySet::new(Policy::Tokenize);

    let mut masked_samples = 0u32;
    let mut failures = Vec::new();

    for sample in &samples {
        let report = detect(&sample.text, &cfg).unwrap();
        let out = mask_report(&sample.text, &report, &policies);

        if !out.applied.is_empty() {
            masked_samples += 1;
        }
        assert!(out.skipped.is_empty(), "겹친 구간이 나왔다: {}", sample.text);

        let map = out.restore.as_ref().expect("토큰화는 복원 맵을 낸다");
        match unmask(&out.text, map) {
            Ok(restored) if restored == sample.text => {}
            Ok(restored) => failures.push(format!("복원 불일치\n  원문: {}\n  복원: {restored}", sample.text)),
            Err(err) => failures.push(format!("복원 실패 {err}\n  원문: {}", sample.text)),
        }
    }

    println!("\n=== 마스킹 왕복 실측 ===");
    println!("표본 {} 건 중 실제로 가려진 표본 {masked_samples} 건", samples.len());
    println!("원문 복원 실패 {} 건\n", failures.len());

    assert!(failures.is_empty(), "복원 실패가 났다:\n{}", failures.join("\n"));
    assert!(masked_samples > 0, "아무것도 가려지지 않았다면 이 시험은 의미가 없다");
}

#[test]
fn every_policy_leaves_bytes_outside_findings_untouched() {
    let samples = corpus(SEED, 8);
    let cfg = Config::default();

    let policies = [
        PolicySet::new(Policy::Redact(rust_pii_transformer::mask::Redaction::Label)),
        PolicySet::new(Policy::Redact(rust_pii_transformer::mask::Redaction::Fill('*'))),
        PolicySet::new(Policy::Partial { keep_prefix: 2, keep_suffix: 2, fill: '*' }),
        PolicySet::new(Policy::Tokenize),
    ];

    for sample in &samples {
        let report = detect(&sample.text, &cfg).unwrap();
        if report.findings.is_empty() {
            continue;
        }

        for policies in &policies {
            let out = mask_report(&sample.text, &report, policies);

            // 첫 탐지 구간 앞과 마지막 탐지 구간 뒤는 원문과 바이트 단위로 같아야 한다.
            let first = out.applied.first().unwrap().source.byte.start as usize;
            let last = out.applied.last().unwrap().source.byte.end as usize;
            assert_eq!(
                &out.text[..first],
                &sample.text[..first],
                "탐지 구간 앞이 훼손됐다: {}",
                sample.text
            );
            let tail_len = sample.text.len() - last;
            assert_eq!(
                &out.text[out.text.len() - tail_len..],
                &sample.text[last..],
                "탐지 구간 뒤가 훼손됐다: {}",
                sample.text
            );
        }
    }
}

#[test]
fn the_corpus_itself_is_reproducible() {
    // 수치를 신뢰하려면 코퍼스가 먼저 재현 가능해야 한다.
    assert_eq!(corpus(SEED, 3), corpus(SEED, 3));
    let sample_count = corpus(SEED, ROUNDS).len();
    assert!(sample_count > 1000, "표본이 {sample_count} 건뿐이라 수치의 신뢰구간이 넓다");
    let _ = Variant::Plain;
}
