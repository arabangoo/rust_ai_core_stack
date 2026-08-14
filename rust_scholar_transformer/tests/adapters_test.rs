//! arXiv OAI-PMH · 유튜브 · 웹(Brave) 어댑터 통합 테스트. 라이브 호출 대신 wiremock 모킹.

use rust_scholar_transformer::{
    ArxivOaiSource, BraveProvider, SearchQuery, Source, SourceKind, WebSource, YoutubeSource,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const OAI: &str = r#"<?xml version="1.0"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
 <ListRecords>
  <record>
   <header><identifier>oai:arXiv.org:2401.00001</identifier><datestamp>2024-01-02</datestamp></header>
   <metadata>
    <oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
     <dc:title>Agentic retrieval methods</dc:title>
     <dc:creator>Doe, Jane</dc:creator>
     <dc:description>We study agentic retrieval.</dc:description>
     <dc:date>2024-01-01</dc:date>
     <dc:identifier>http://arxiv.org/abs/2401.00001v1</dc:identifier>
    </oai_dc:dc>
   </metadata>
  </record>
  <record>
   <header><identifier>oai:arXiv.org:2401.00002</identifier><datestamp>2024-01-03</datestamp></header>
   <metadata>
    <oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
     <dc:title>Cooking recipes dataset</dc:title>
     <dc:creator>Smith, Sam</dc:creator>
     <dc:description>A dataset of recipes.</dc:description>
     <dc:date>2024-01-01</dc:date>
     <dc:identifier>http://arxiv.org/abs/2401.00002v1</dc:identifier>
    </oai_dc:dc>
   </metadata>
  </record>
 </ListRecords>
</OAI-PMH>"#;

const YT: &str = r#"{
  "items": [
    {"id": {"videoId": "abc123"},
     "snippet": {"title": "Agentic AI explained", "description": "A talk.", "channelTitle": "AI Channel", "publishedAt": "2024-01-01T00:00:00Z"}},
    {"id": {"videoId": "def456"},
     "snippet": {"title": "Another video", "description": "", "channelTitle": "Chan", "publishedAt": "2024-02-01T00:00:00Z"}}
  ]
}"#;

const BRAVE: &str = r#"{
  "web": {
    "results": [
      {"title": "Agentic loops guide", "url": "https://example.com/guide?utm_source=brave", "description": "<b>Guide</b>", "page_age": "2024-01-01T00:00:00Z"},
      {"title": "Second result", "url": "https://example.com/second", "description": "More"}
    ]
  }
}"#;

#[tokio::test]
async fn arxiv_oai_harvests_and_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oai"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OAI))
        .mount(&server)
        .await;

    let src = ArxivOaiSource::new().with_base_url(format!("{}/oai", server.uri()));
    let docs = src.search(&SearchQuery::from_text("agentic", 20)).await.unwrap();

    // "agentic" 키워드 필터 -> 요리 레코드는 제외, 1건.
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].source, SourceKind::Arxiv);
    assert_eq!(docs[0].identity.arxiv_id.as_deref(), Some("2401.00001"));
    assert_eq!(docs[0].authors.len(), 1);
    assert!(docs[0].published_at.is_some());
}

/// 0.1.x 회귀 방어. 필터가 OR 이던 시절에는 낱말 하나만 걸려도 통과해서 질의와 상관없는
/// 레코드가 결과로 나왔다. 위 테스트는 단어 하나짜리 질의라 그 결함을 잡지 못했다.
#[tokio::test]
async fn arxiv_oai_requires_all_terms_not_any() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oai"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OAI))
        .mount(&server)
        .await;
    let src = ArxivOaiSource::new().with_base_url(format!("{}/oai", server.uri()));

    // "agentic" 은 1번 레코드에만, "recipes" 는 2번 레코드에만 있다.
    // OR 이면 두 건이 다 나오고, AND 면 두 낱말을 함께 가진 레코드가 없어 0건이어야 한다.
    let docs = src.search(&SearchQuery::from_text("agentic recipes", 20)).await.unwrap();
    assert!(docs.is_empty(), "AND 매칭이면 교집합이 없어 0건이어야 한다");

    // 같은 레코드 안에 둘 다 있는 조합은 정상적으로 잡힌다.
    let docs = src.search(&SearchQuery::from_text("agentic retrieval", 20)).await.unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].identity.arxiv_id.as_deref(), Some("2401.00001"));
}

/// 두 글자 이하 토큰은 필터에서 뺀다. 안 그러면 관사·전치사 하나가 AND 조건을 무의미하게 만든다.
#[tokio::test]
async fn arxiv_oai_ignores_short_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oai"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OAI))
        .mount(&server)
        .await;
    let src = ArxivOaiSource::new().with_base_url(format!("{}/oai", server.uri()));

    // "of" 는 무시되고 "agentic" 만 조건으로 남는다.
    let docs = src.search(&SearchQuery::from_text("of agentic", 20)).await.unwrap();
    assert_eq!(docs.len(), 1);
}

#[tokio::test]
async fn youtube_parses_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/youtube/v3/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(YT))
        .mount(&server)
        .await;

    let src = YoutubeSource::new("test-key")
        .with_base_url(format!("{}/youtube/v3/search", server.uri()));
    let docs = src.search(&SearchQuery::from_text("agentic", 10)).await.unwrap();

    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].source, SourceKind::Youtube);
    assert_eq!(docs[0].url, "https://www.youtube.com/watch?v=abc123");
    assert_eq!(docs[0].authors[0].name, "AI Channel");
}

#[tokio::test]
async fn youtube_requires_api_key() {
    let src = YoutubeSource::new("");
    assert!(src.search(&SearchQuery::from_text("x", 5)).await.is_err());
}

#[tokio::test]
async fn web_brave_provider_parses_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/res/v1/web/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(BRAVE))
        .mount(&server)
        .await;

    let provider = BraveProvider::new("test-key")
        .with_base_url(format!("{}/res/v1/web/search", server.uri()));
    let src = WebSource::new(Box::new(provider));
    let docs = src.search(&SearchQuery::from_text("agentic loops", 10)).await.unwrap();

    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].source, SourceKind::Web);
    // utm 제거된 canonical URL + HTML 정제된 요약.
    assert_eq!(docs[0].identity.canonical_url.as_deref(), Some("https://example.com/guide"));
    assert_eq!(docs[0].summary.as_deref(), Some("Guide"));
}
