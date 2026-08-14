//! arXiv 어댑터 — 라이브 검색 API(`export.arxiv.org/api/query`) 경로. **기본 경로다(0.2.0).**
//!
//! 0.1.x 까지는 OAI-PMH 미러가 1순위였다. 근거는 "공유 IP 에서 라이브 API 가 HTTP 429 로 막힌다"
//! 였는데, 이 전제는 실측으로 무너졌다(2026-08-14, 연속 3회 전부 HTTP 200 / 평균 0.39초).
//! 반대로 OAI 경로는 날짜 기반 수확이라 **전체 아카이브 검색이 되지 않아** 재현율이 사실상 0 에
//! 가까웠다(음성 인식 질의에 관련 논문 0건). 그래서 기본을 이쪽으로 되돌린다.
//!
//! 라이브 경로의 방어는 그대로다: HTTP status 를 본문 파싱 전에 먼저 확인하고(429 응답 HTML 을
//! XML 로 잘못 파싱하지 않도록), 429 는 Retry-After 존중 + 지수 백오프로 재시도하며, 식별 가능한
//! User-Agent 를 보낸다. OAI 경로는 [`super::arxiv_oai`] 에 폴백 겸 최신 모니터링용으로 남아 있다.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::FetchError;
use crate::model::{Author, DocIdentity, Document, Score, SearchQuery, SourceKind};
use crate::normalize::{canonicalize_url, normalize_title, title_hash};
use crate::source::{RatePolicy, Source};

const DEFAULT_BASE_URL: &str = "https://export.arxiv.org/api/query";
const DEFAULT_USER_AGENT: &str =
    "rust_scholar_transformer/0.1 (+https://github.com/arabangoo/rust_scholar_transformer)";

/// arXiv 라이브 검색 API 어댑터.
pub struct ArxivSource {
    client: reqwest::Client,
    base_url: String,
    user_agent: String,
    max_retries: u32,
    retry_base_delay: Duration,
    /// arXiv 분류 한정(예: `["cs.CL", "eess.AS"]`). 비어 있으면 전 분야.
    categories: Vec<String>,
}

impl ArxivSource {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            max_retries: 3,
            retry_base_delay: Duration::from_secs(3),
            categories: Vec::new(),
        }
    }

    /// 분류를 한정한다(예: `["cs.CL", "eess.AS"]`). 여러 개면 OR 로 묶여 질의에 AND 결합된다.
    /// 관심 분야를 좁히면 같은 개수를 받아도 관련 논문 밀도가 올라간다.
    pub fn with_categories(mut self, categories: Vec<String>) -> Self {
        self.categories = categories.into_iter().filter(|c| !c.trim().is_empty()).collect();
        self
    }

    /// 베이스 URL 교체(테스트의 모킹 서버 등).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// 재시도 횟수·기본 백오프 간격 설정(테스트에서 0 으로 빠르게).
    pub fn with_retry(mut self, max_retries: u32, base_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.retry_base_delay = base_delay;
        self
    }

    /// 연락처를 User-Agent 에 넣는다(arXiv 권장). 개인 이메일은 코드에 하드코딩하지 않고 주입.
    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    async fn fetch(&self, q: &SearchQuery) -> Result<String, FetchError> {
        let search_query = build_search_query(&q.text, &self.categories);
        let max_results = q.limit.max(1).to_string();
        let params = [
            ("search_query", search_query.as_str()),
            ("start", "0"),
            ("max_results", max_results.as_str()),
            ("sortBy", "submittedDate"),
            ("sortOrder", "descending"),
        ];

        let mut attempt = 0u32;
        loop {
            let resp = self
                .client
                .get(&self.base_url)
                .header(reqwest::header::USER_AGENT, self.user_agent.as_str())
                .header(reqwest::header::ACCEPT, "application/atom+xml,text/xml,*/*")
                .query(&params)
                .send()
                .await
                .map_err(|e| FetchError::Http(e.to_string()))?;

            let status = resp.status();

            // status 를 본문 파싱 전에 먼저 본다(검증된 교훈). 429 면 백오프 후 재시도.
            if status.as_u16() == 429 && attempt < self.max_retries {
                let wait = retry_after(&resp).unwrap_or_else(|| backoff(self.retry_base_delay, attempt));
                attempt += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Http(format!("arXiv HTTP {}", status.as_u16())));
            }
            return resp.text().await.map_err(|e| FetchError::Http(e.to_string()));
        }
    }

    fn parse_and_normalize(&self, xml: &str) -> Result<Vec<Document>, FetchError> {
        let feed: AtomFeed =
            quick_xml::de::from_str(xml).map_err(|e| FetchError::Parse(e.to_string()))?;
        let now = Utc::now();
        Ok(feed.entries.into_iter().map(|e| entry_to_doc(e, now)).collect())
    }
}

impl Default for ArxivSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Source for ArxivSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Arxiv
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<Document>, FetchError> {
        if query.text.trim().is_empty() {
            return Err(FetchError::InvalidQuery("empty query text".to_string()));
        }
        let xml = self.fetch(query).await?;
        self.parse_and_normalize(&xml)
    }

    fn rate_policy(&self) -> RatePolicy {
        // arXiv 권장: 요청 간 3초 + 동시 연결 1.
        RatePolicy { min_interval_ms: 3000, max_concurrency: 1, daily_quota: None }
    }
}

// ── Atom 역직렬화 구조체 (quick-xml serde) ───────────────────────────

#[derive(Debug, Deserialize)]
struct AtomFeed {
    #[serde(rename = "entry", default)]
    entries: Vec<AtomEntry>,
}

#[derive(Debug, Deserialize)]
struct AtomEntry {
    #[serde(default)]
    title: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    published: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(rename = "author", default)]
    authors: Vec<AtomAuthor>,
}

#[derive(Debug, Deserialize)]
struct AtomAuthor {
    #[serde(default)]
    name: String,
}

// ── 질의 빌더 ────────────────────────────────────────────────────────

/// 자유 텍스트를 arXiv 검색 문법으로 옮긴다.
///
/// 규칙 셋이다.
/// 1. 큰따옴표로 감싼 부분은 **구문 검색**으로 보존한다(`"speaker diarization"`).
/// 2. 나머지 토큰은 **AND** 로 결합한다. 0.1.x 는 `all:<원문>` 을 그대로 넘겼는데, 그러면
///    낱말 하나만 걸려도 통과해 관련 없는 결과가 섞였다. 정밀도를 기본으로 둔다.
/// 3. 분류가 지정되면 `cat:` 절을 OR 로 묶어 전체에 AND 결합한다.
///
/// 토큰을 AND 로 묶으므로 **긴 자연어 문장을 그대로 넣으면 0건이 나올 수 있다.** 이는 결함이
/// 아니라 의도된 계약이며, 0건은 [`crate::error::SearchReport`] 의 경고로 드러난다. 호출자는
/// 핵심어 두셋이나 따옴표 구문으로 질의를 좁혀서 넣는다.
fn build_search_query(text: &str, categories: &[String]) -> String {
    let mut clauses: Vec<String> = Vec::new();
    let mut rest = String::new();
    let mut phrase = String::new();
    let mut in_quote = false;

    for ch in text.chars() {
        match ch {
            '"' if in_quote => {
                let p = phrase.trim();
                if !p.is_empty() {
                    clauses.push(format!("all:\"{p}\""));
                }
                phrase.clear();
                in_quote = false;
            }
            '"' => in_quote = true,
            _ if in_quote => phrase.push(ch),
            _ => rest.push(ch),
        }
    }
    // 닫히지 않은 따옴표는 일반 토큰으로 되돌린다(질의를 통째로 잃지 않게).
    if !phrase.trim().is_empty() {
        rest.push(' ');
        rest.push_str(&phrase);
    }

    for tok in rest.split_whitespace() {
        let t = sanitize_token(tok);
        if t.is_empty() {
            continue;
        }
        clauses.push(format!("all:{t}"));
    }

    let core = if clauses.is_empty() {
        format!("all:{}", text.trim())
    } else {
        clauses.join(" AND ")
    };

    if categories.is_empty() {
        core
    } else {
        let cats: Vec<String> = categories.iter().map(|c| format!("cat:{}", c.trim())).collect();
        format!("({}) AND ({})", core, cats.join(" OR "))
    }
}

/// arXiv 질의 문법에서 의미를 갖는 문자를 떼어낸다. 괄호·콜론·따옴표가 토큰에 섞여 들어가면
/// 질의 전체가 문법 오류가 되어 조용히 빈 결과를 낸다.
fn sanitize_token(tok: &str) -> String {
    tok.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '\''))
        .collect()
}

// ── 정규화 헬퍼 ──────────────────────────────────────────────────────

fn entry_to_doc(e: AtomEntry, now: DateTime<Utc>) -> Document {
    let title = normalize_title(&e.title);
    let arxiv_id = arxiv_id_from_id_url(&e.id);
    let canonical_url = canonicalize_url(&e.id);
    let published_at = e
        .published
        .as_deref()
        .and_then(|p| DateTime::parse_from_rfc3339(p).ok())
        .map(|d| d.with_timezone(&Utc));
    let authors = e
        .authors
        .into_iter()
        .filter(|a| !a.name.trim().is_empty())
        .map(|a| Author::new(normalize_title(&a.name)))
        .collect();

    let identity = DocIdentity {
        doi: None,
        arxiv_id,
        canonical_url,
        title_hash: title_hash(&title),
    };

    Document {
        identity,
        source: SourceKind::Arxiv,
        title,
        url: e.id,
        authors,
        published_at,
        fetched_at: now,
        summary: e.summary.map(|s| normalize_title(&s)),
        content: None,
        language: Some("en".to_string()),
        tags: Vec::new(),
        sources: vec![SourceKind::Arxiv],
        score: Score::default(),
        extra: HashMap::new(),
    }
}

/// `http://arxiv.org/abs/2301.00001v2` -> `2301.00001` (버전 접미사 제거).
fn arxiv_id_from_id_url(id_url: &str) -> Option<String> {
    let after = id_url.split("/abs/").nth(1)?;
    let core = match after.rfind('v') {
        Some(pos)
            if pos + 1 < after.len() && after[pos + 1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            &after[..pos]
        }
        _ => after,
    };
    Some(core.to_string())
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let v = resp.headers().get(reqwest::header::RETRY_AFTER)?;
    let secs: u64 = v.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    let factor = 1u32 << attempt.min(5);
    base.saturating_mul(factor).min(Duration::from_secs(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_arxiv_id_versions() {
        assert_eq!(
            arxiv_id_from_id_url("http://arxiv.org/abs/2301.00001v2").as_deref(),
            Some("2301.00001")
        );
        assert_eq!(
            arxiv_id_from_id_url("http://arxiv.org/abs/2302.12345").as_deref(),
            Some("2302.12345")
        );
        // 구식 식별자(슬래시 포함)도 버전만 제거.
        assert_eq!(
            arxiv_id_from_id_url("http://arxiv.org/abs/math.GT/0309136v1").as_deref(),
            Some("math.GT/0309136")
        );
        assert_eq!(arxiv_id_from_id_url("https://example.com/x").as_deref(), None);
    }

    #[test]
    fn builds_and_joined_query() {
        // 0.1.x 는 `all:speech recognition` 을 그대로 넘겨 낱말 하나만 걸려도 통과했다.
        assert_eq!(
            build_search_query("speech recognition", &[]),
            "all:speech AND all:recognition"
        );
    }

    #[test]
    fn preserves_quoted_phrases() {
        assert_eq!(
            build_search_query("\"speaker diarization\" whisper", &[]),
            "all:\"speaker diarization\" AND all:whisper"
        );
    }

    #[test]
    fn appends_category_filter_as_or_group() {
        assert_eq!(
            build_search_query("whisper", &["cs.CL".to_string(), "eess.AS".to_string()]),
            "(all:whisper) AND (cat:cs.CL OR cat:eess.AS)"
        );
    }

    #[test]
    fn strips_syntax_breaking_characters() {
        // 괄호·콜론이 토큰에 남으면 질의 전체가 문법 오류가 되어 조용히 0건이 된다.
        assert_eq!(build_search_query("ASR (2026):", &[]), "all:ASR AND all:2026");
        assert_eq!(sanitize_token("end-to-end"), "end-to-end");
    }

    #[test]
    fn recovers_unclosed_quote_as_plain_tokens() {
        assert_eq!(
            build_search_query("\"speaker diarization", &[]),
            "all:speaker AND all:diarization"
        );
    }

    #[test]
    fn parses_atom_into_documents() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/2301.00001v2</id>
    <title>Sample One</title>
    <published>2023-01-01T00:00:00Z</published>
    <summary>S1.</summary>
    <author><name>Alice</name></author>
    <author><name>Bob</name></author>
  </entry>
</feed>"#;
        let src = ArxivSource::new();
        let docs = src.parse_and_normalize(xml).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "Sample One");
        assert_eq!(docs[0].identity.arxiv_id.as_deref(), Some("2301.00001"));
        assert_eq!(docs[0].authors.len(), 2);
        assert!(docs[0].published_at.is_some());
    }
}
