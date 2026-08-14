//! 실제 네트워크를 치는 연막 시험. 기본적으로 `#[ignore]` 라 `cargo test` 에서는 돌지 않는다.
//!
//! ```bash
//! cargo test --test live_smoke -- --ignored --nocapture
//! ```
//!
//! **왜 필요한가.** 0.1.x 의 arXiv 재현율 결함(질의와 무관한 논문만 반환)은 단위·통합 테스트를
//! 전부 통과한 채로 출시됐다. 모든 테스트가 wiremock 목이라 **어댑터가 실제 서비스에 맞게
//! 질의를 만드는지**를 아무도 확인하지 않았기 때문이다. 목은 파서를 검증하고, 이 파일은
//! 질의 문법과 응답 계약을 검증한다. 둘은 다른 것을 본다.
//!
//! CI 에서는 외부 서비스 상태에 결과가 묶이므로 기본 제외한다. 릴리스 전에 손으로 한 번 돌린다.

use std::time::{Duration, Instant};

use rust_scholar_transformer::{ArxivOaiSource, ArxivSource, SearchQuery, Source, SourceKind};

/// 엔진의 소스별 기본 타임아웃. 어댑터는 이 안에서 끝나야 한다.
const ENGINE_DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// arXiv 라이브 검색 API 가 살아 있고, 질의어와 실제로 관련된 결과를 주는지 본다.
#[tokio::test]
#[ignore = "네트워크 필요. cargo test --test live_smoke -- --ignored"]
async fn arxiv_live_returns_on_topic_results() {
    let src = ArxivSource::new();
    let docs = src
        .search(&SearchQuery::from_text("\"speaker diarization\"", 10))
        .await
        .expect("arXiv 라이브 API 호출 실패");

    assert!(!docs.is_empty(), "구문 검색이 0건이면 질의 빌더나 API 계약이 깨진 것이다");
    assert!(docs.iter().all(|d| d.source == SourceKind::Arxiv));
    assert!(docs.iter().all(|d| d.identity.arxiv_id.is_some()), "arXiv ID 정규화 실패");

    // 재현율 회귀 방어의 핵심. 0.1.x 는 여기서 0 이 나왔다.
    let on_topic = docs
        .iter()
        .filter(|d| {
            let hay = format!("{} {}", d.title, d.summary.clone().unwrap_or_default()).to_lowercase();
            hay.contains("diariz") || hay.contains("speaker")
        })
        .count();
    assert!(
        on_topic * 2 >= docs.len(),
        "관련 결과가 절반 미만이다({on_topic}/{}). 질의가 느슨해졌는지 확인할 것",
        docs.len()
    );

    for d in docs.iter().take(3) {
        println!("  {} {}", d.published_at.map(|p| p.format("%Y-%m-%d").to_string()).unwrap_or_default(), d.title);
    }
}

/// 분류 한정이 실제 API 문법으로 통하는지 본다(`cat:` 절이 틀리면 조용히 0건이 된다).
#[tokio::test]
#[ignore = "네트워크 필요. cargo test --test live_smoke -- --ignored"]
async fn arxiv_live_category_filter_narrows_without_emptying() {
    let src = ArxivSource::new().with_categories(vec!["eess.AS".to_string(), "cs.CL".to_string()]);
    let docs = src
        .search(&SearchQuery::from_text("\"speech recognition\"", 10))
        .await
        .expect("arXiv 라이브 API 호출 실패");

    assert!(
        !docs.is_empty(),
        "분류 한정이 0건을 만들면 cat: 절 문법이 틀린 것이다(문법 오류는 오류가 아니라 빈 결과로 나온다)"
    );
    for d in docs.iter().take(3) {
        println!("  {} {}", d.published_at.map(|p| p.format("%Y-%m-%d").to_string()).unwrap_or_default(), d.title);
    }
}

/// 매칭이 없으면 정말로 0건이 나오는지 본다. 0.1.x 는 무의미 질의에도 결과를 냈다.
#[tokio::test]
#[ignore = "네트워크 필요. cargo test --test live_smoke -- --ignored"]
async fn arxiv_live_nonsense_query_returns_nothing() {
    let src = ArxivSource::new();
    let docs = src
        .search(&SearchQuery::from_text(
            "\"zzqqxx nonexistent phrase that cannot appear\"",
            10,
        ))
        .await
        .expect("arXiv 라이브 API 호출 실패");

    assert!(docs.is_empty(), "없는 것을 없다고 말하지 못하면 검색기가 아니다");
}

/// OAI 경로가 엔진 기본 타임아웃 안에서 끝나는지 본다.
///
/// 회귀 이력: 페이지네이션을 넣으면서 기본 페이지 수를 3 으로 뒀더니 한 페이지가 약 3.6MB /
/// 1300건이라 8.5초에서 10초를 넘겼고, 엔진이 이 소스를 통째로 timeout 경고로 떨궜다. 목
/// 테스트는 응답이 작아서 이 비용을 볼 수 없었다. **어댑터의 시간 예산은 실물로만 측정된다.**
#[tokio::test]
#[ignore = "네트워크 필요. cargo test --test live_smoke -- --ignored"]
async fn arxiv_oai_live_fits_in_engine_timeout() {
    let src = ArxivOaiSource::new().with_set("cs").with_from_days(2);

    let t0 = Instant::now();
    let docs = src
        .search(&SearchQuery::from_text("speech recognition", 20))
        .await
        .expect("arXiv OAI 호출 실패");
    let elapsed = t0.elapsed();

    println!("  OAI 소요 {:.2}초, {}건", elapsed.as_secs_f64(), docs.len());
    assert!(
        elapsed < ENGINE_DEFAULT_TIMEOUT,
        "OAI 가 {:.2}초 걸렸다. 엔진 기본 타임아웃 {}초를 넘으면 이 소스는 결과가 아니라 경고가 된다",
        elapsed.as_secs_f64(),
        ENGINE_DEFAULT_TIMEOUT.as_secs()
    );
}
