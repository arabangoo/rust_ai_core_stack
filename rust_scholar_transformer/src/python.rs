//! PyO3 바인딩 — `feature = "python"` 활성 시 cdylib 으로 빌드되어 `import rust_scholar_transformer`
//! 로 사용한다. abi3(Python 3.9+) 단일 휠. 동기 우선(sync-first): 내부 tokio 런타임에서 block_on
//! 으로 완료시켜 일반 함수처럼 노출한다(asyncio/Jupyter 환경 차이 회피). 결과는 JSON 문자열.
//!
//! ```python
//! from rust_scholar_transformer import Retriever
//! r = Retriever(sources=["arxiv", "news"])
//! docs = r.search("agentic loop engineering", limit=20)  # JSON 문자열
//! ```

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use crate::sources::{
    ArxivOaiSource, ArxivSource, BraveProvider, FeedSource, GoogleNewsSource, RssSource, WebSource,
    YoutubeSource,
};
use crate::{Engine, SearchQuery};

#[pyclass]
struct Retriever {
    engine: Engine,
    rt: tokio::runtime::Runtime,
}

#[pymethods]
impl Retriever {
    /// 소스 목록과 자격증명으로 리트리버를 만든다.
    ///
    /// sources: `"arxiv"` | `"arxiv_oai"` | `"news"` | `"blog"` | `"youtube"` | `"web"`
    /// (기본 `["arxiv","news"]`).
    ///
    /// **0.2.0 의 동작 변경.** `"arxiv"` 가 이제 라이브 검색 API([`ArxivSource`])를 등록한다.
    /// 0.1.x 에서는 OAI-PMH 수확기가 등록됐고, 그 경로는 최근 N일 창 밖을 원리적으로 찾지
    /// 못해 일반 검색의 재현율이 사실상 0 이었다. 최근 구간 전량 모니터링이 필요하면
    /// `"arxiv_oai"` 를 명시적으로 지정한다.
    ///
    /// - `arxiv_categories`: 라이브 API 의 분류 한정(예: `["cs.CL", "eess.AS"]`).
    /// - `arxiv_oai_set`: OAI 경로의 set 한정(예: `"cs"`). 지정하지 않으면 전 분야를 수확한다.
    /// - `arxiv_oai_days`: OAI 경로의 수확 창(기본 7일).
    /// - `arxiv_oai_max_pages`: OAI 경로가 따라갈 페이지 수(기본 1). 페이지당 약 3.6MB /
    ///   1300건이라 올리면 `timeout_secs` 도 함께 올려야 한다.
    /// - `timeout_secs`: 소스별 타임아웃(기본 10초). OAI 를 여러 페이지 훑을 때 올린다.
    #[new]
    #[pyo3(signature = (
        sources=None,
        rss_feeds=None,
        youtube_api_key=None,
        brave_api_key=None,
        arxiv_categories=None,
        arxiv_oai_set=None,
        arxiv_oai_days=None,
        arxiv_oai_max_pages=None,
        timeout_secs=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        sources: Option<Vec<String>>,
        rss_feeds: Option<Vec<String>>,
        youtube_api_key: Option<String>,
        brave_api_key: Option<String>,
        arxiv_categories: Option<Vec<String>>,
        arxiv_oai_set: Option<String>,
        arxiv_oai_days: Option<i64>,
        arxiv_oai_max_pages: Option<usize>,
        timeout_secs: Option<u64>,
    ) -> PyResult<Self> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let mut engine = Engine::new();
        if let Some(secs) = timeout_secs {
            engine = engine.with_timeout(std::time::Duration::from_secs(secs.max(1)));
        }
        let wanted = sources.unwrap_or_else(|| vec!["arxiv".to_string(), "news".to_string()]);

        for s in &wanted {
            match s.as_str() {
                "arxiv" => {
                    let mut src = ArxivSource::new();
                    if let Some(cats) = &arxiv_categories {
                        src = src.with_categories(cats.clone());
                    }
                    engine.register(Box::new(src));
                }
                "arxiv_oai" => {
                    let mut src = ArxivOaiSource::new();
                    if let Some(set) = &arxiv_oai_set {
                        src = src.with_set(set.clone());
                    }
                    if let Some(days) = arxiv_oai_days {
                        src = src.with_from_days(days);
                    }
                    if let Some(pages) = arxiv_oai_max_pages {
                        src = src.with_max_pages(pages);
                    }
                    engine.register(Box::new(src));
                }
                "news" => {
                    engine.register(Box::new(GoogleNewsSource::new()));
                }
                "blog" => {
                    if let Some(feeds) = &rss_feeds {
                        let fs = feeds.iter().map(|u| FeedSource::new("feed", u.clone())).collect();
                        engine.register(Box::new(RssSource::new(fs)));
                    }
                }
                "youtube" => {
                    if let Some(k) = &youtube_api_key {
                        engine.register(Box::new(YoutubeSource::new(k.clone())));
                    }
                }
                "web" => {
                    if let Some(k) = &brave_api_key {
                        engine.register(Box::new(WebSource::new(Box::new(BraveProvider::new(
                            k.clone(),
                        )))));
                    }
                }
                _ => {}
            }
        }
        Ok(Self { engine, rt })
    }

    /// 질의를 실행하고 결과를 JSON 문자열로 돌려준다(동기). 내부에서 동시 fan-out + 융합 + 중복제거.
    #[pyo3(signature = (query, limit=20))]
    fn search(&self, query: &str, limit: usize) -> PyResult<String> {
        let q = SearchQuery::from_text(query, limit);
        let report = self.rt.block_on(self.engine.search(q));
        serde_json::to_string(&report).map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }
}

#[pymodule]
fn rust_scholar_transformer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<Retriever>()?;
    Ok(())
}
