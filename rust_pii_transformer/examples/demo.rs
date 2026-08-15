//! 상담 기록 형태의 문서에서 무엇이 잡히고 무엇이 안 잡히는지 눈으로 확인하는 예제.
//!
//! ```bash
//! cargo run --example demo
//! ```
//!
//! 판정된 결과뿐 아니라 **떨어진 후보와 그 사유**도 함께 낸다. 오탐이 없다는 것과 미탐이
//! 없다는 것은 다른 주장이고, 둘 다 눈으로 확인할 수 있어야 하기 때문이다.

use rust_pii_transformer::detect::{detect, Config};

const DOC: &str = "고객 상담 기록\n\
김민수 고객님(주민등록번호 880101-1234568)께서 문의하셨습니다.\n\
연락처는 010-1234-5678 이고 이메일은 minsu.kim@example.com 입니다.\n\
결제 카드번호 4111-1111-1111-1111 로 승인되었으며,\n\
환불 계좌번호는 국민은행 1234567890123 입니다.\n\
사업자등록번호 124-81-00998 로 세금계산서를 발행했습니다.\n\
외국인 동반자의 외국인등록번호는 팔팔공일공일 - 오이삼사오육칠 입니다.\n\
참고로 주문번호 20240115003 과 송장번호 987654321098 은 개인정보가 아닙니다.\n\
제품 1234 수량 5678 개, 결제 금액 일억이천만원.";

fn main() {
    let report = detect(DOC, &Config::default()).unwrap();

    println!("{}", "=".repeat(78));
    println!("탐지 결과 {}건", report.findings.len());
    println!("{}", "=".repeat(78));
    for finding in &report.findings {
        println!(
            "[{:?}] {:<16} 점수 {:.2}  원문 {:?}",
            finding.certainty,
            finding.entity.label(),
            finding.score,
            finding.source.slice(DOC)
        );
        println!(
            "     규칙 {} / 검증 {:?}",
            finding.evidence.rule, finding.evidence.checksum
        );
        if !finding.evidence.context_hits.is_empty() {
            let cues: Vec<String> = finding
                .evidence
                .context_hits
                .iter()
                .map(|h| format!("{}({}자)", h.cue, h.distance))
                .collect();
            println!("     문맥 {}", cues.join(", "));
        }
        let cost = &finding.evidence.cost;
        if !cost.is_zero() {
            println!(
                "     정규화 비용 공백{} 구분자{} 음절{} 치환{}",
                cost.absorbed_whitespace,
                cost.absorbed_separators,
                cost.expanded_syllables,
                cost.replaced_chars
            );
        }
        println!();
    }

    println!("{}", "-".repeat(78));
    println!("떨어진 후보 {}건 (왜 안 걸렸는가)", report.rejections.len());
    println!("{}", "-".repeat(78));
    for rejection in &report.rejections {
        println!(
            "  {:<16} {:?}  점수 {:.2}  원문 {:?}",
            rejection.entity.label(),
            rejection.reason,
            rejection.score,
            rejection.source.slice(DOC)
        );
    }
}
