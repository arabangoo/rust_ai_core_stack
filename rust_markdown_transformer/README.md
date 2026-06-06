# rust_markdown_transformer

> **Rust 기반 만능 문서 → Markdown 변환 플러그인 라이브러리**
>
> `docx · pptx · xlsx · hwpx · pdf · html · markdown` 문서를
> **벡터 DB / RAG 친화적인 Markdown** 으로 결정적(deterministic)이고 빠르게 변환한다.

이 문서는 라이브러리의 **완결된 개발자 매뉴얼**이다. 설계 철학, 공개 API, 지원 포맷별 동작과 한계,
CLI/Python 사용법, 새 포맷 추가 방법, 빌드/테스트 절차를 모두 담는다.

[주요 참고 논문]

1. XY-Cut++: Advanced Layout Ordering via Hierarchical Mask Mechanism on a Novel Benchmark - https://arxiv.org/abs/2504.10258
2. LayoutReader: Pre-training of Text and Layout for Reading Order Detection - https://arxiv.org/abs/2108.11591
3. Nougat: Neural Optical Understanding for Academic Documents - https://arxiv.org/abs/2308.13418

---

## 목차

1. [핵심 특징](#1-핵심-특징)
2. [빠른 시작](#2-빠른-시작)
3. [설치와 Cargo Feature](#3-설치와-cargo-feature)
4. [아키텍처](#4-아키텍처)
5. [공통 IR 레퍼런스](#5-공통-ir-레퍼런스)
6. [공개 API 레퍼런스](#6-공개-api-레퍼런스)
7. [지원 포맷별 동작과 한계](#7-지원-포맷별-동작과-한계)
8. [Semantic Chunking](#8-semantic-chunking)
9. [CLI 도구 (`rmt`)](#9-cli-도구-rmt)
10. [Python 바인딩 (PyO3)](#10-python-바인딩-pyo3)
11. [서비스 파이프라인에 붙이기 (통합 레시피)](#11-서비스-파이프라인에-붙이기-통합-레시피)
12. [새 포맷 파서 추가하기](#12-새-포맷-파서-추가하기)
13. [빌드 · Feature 조합 · 테스트](#13-빌드--feature-조합--테스트)
14. [디렉토리 구조](#14-디렉토리-구조)
15. [라이선스](#15-라이선스)

---

## 1. 핵심 특징

RAG / 벡터 DB 파이프라인에서 가장 과소평가되는 영역이 **문서 ingestion** 이다. 모델 품질이 아무리 좋아도
입력 문서 처리 품질이 낮으면 검색·답변이 무너진다. 이 라이브러리는 단순 텍스트 추출기가 아니라
**"인덱싱 품질을 최대화하는 구조 보존형 변환 엔진"** 을 지향한다.

| 원칙 | 의미 |
|---|---|
| **Deterministic** | 같은 입력 → 항상 같은 출력. 캐싱·테스트·디버깅이 쉽다. ML 기반 도구를 1차 선택지에서 제외한 이유. |
| **Structure-preserving** | 단순 텍스트 덤프가 아니라 **제목 계층·표·목록·코드블록·링크·강조**를 Markdown 문법으로 재현. |
| **Plugin-extensible** | 새 포맷은 [`FormatParser`](#61-formatparser-트레이트) 트레이트 하나만 구현하면 끝. 코어를 건드리지 않는다. |
| **Zero-dependency self-contained** | 기본 빌드는 **pure Rust / zero FFI / zero subprocess**. `Cargo.toml` 한 줄 추가만으로 안심하고 붙인다. npm·JVM·Python 런타임을 요구하지 않는다. |

### 왜 Markdown-first 인가

Markdown 은 벡터 DB 청킹의 사실상 표준이다.

- **헤딩(`#`, `##`)이 의미 경계의 universal marker** — LangChain `MarkdownHeaderTextSplitter`,
  LlamaIndex `MarkdownNodeParser` 등 대부분의 청커가 헤딩을 1급 시민으로 취급한다.
- **LLM 이 가장 잘 이해하는 텍스트 포맷** — 검색된 청크를 컨텍스트에 주입했을 때 HTML/XML/raw text 대비
  답변 품질이 일관되게 우위.
- **토큰 효율 + 디버깅 용이** — `.md` 파일을 직접 열어 "임베딩 모델이 본 것" 을 그대로 확인할 수 있다.

Markdown 은 lossy 하다(병합셀·PDF 좌표·이미지 시각의미 손실). 이를 위해 같은 [IR](#5-공통-ir-레퍼런스) 에서
**Markdown(1차 산출물) + IR JSON(안전망)** 두 트랙을 동시에 제공한다.

```text
원본(any format) → [Rust 파서] → IR → 두 트랙으로 분기
    트랙 1 → Markdown (.md)      → 벡터 DB / RAG (99% 케이스)
    트랙 2 → IR JSON (.ir.json) → 멀티모달 RAG / 정밀 citation / lossless 재처리
```

---

## 2. 빠른 시작

### Rust 라이브러리

```rust
use rust_markdown_transformer::{ParserRegistry, SemanticChunker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = ParserRegistry::with_defaults();

    // 1) 단순 변환 — 확장자/매직바이트로 파서 자동 선택
    let md = registry.convert_to_markdown("report.docx".as_ref())?;
    std::fs::write("report.md", md)?;

    // 2) 벡터 DB 적재용 청킹
    let doc = registry.parse_to_ir("report.hwpx".as_ref())?;
    let chunks = SemanticChunker { max_tokens: 512, overlap_tokens: 64, heading_levels: vec![1, 2] }
        .chunk(&doc);
    for c in &chunks {
        println!("{}", serde_json::to_string(c)?);
    }
    Ok(())
}
```

### CLI

```bash
cargo build --release --features cli
./target/release/rmt convert report.pdf -o report.md
```

### Python

```python
import rust_markdown_transformer as rmt
md = rmt.convert_to_markdown("report.hwpx")   # 어떤 포맷이든 한 줄로 정규화
```

---

## 3. 설치와 Cargo Feature

`Cargo.toml`:

```toml
[dependencies]
rust_markdown_transformer = "0.1"
```

### Feature 목록

| Feature | 활성화 대상 | 비고 |
|---|---|---|
| `docx` | DOCX 파서 | `zip`, `quick-xml` |
| `pptx` | PPTX 파서 | `zip`, `quick-xml` |
| `xlsx` | XLSX/XLSM 파서 | `calamine` |
| `hwpx` | 한컴 HWPX(OWPML) 파서 | `zip`, `quick-xml` |
| `pdf` | PDF 파서 | `pdf-extract`(텍스트), `lopdf`(메타데이터) |
| `html` | HTML 파서 | `scraper` |
| `markdown` | Markdown 재정규화 | `pulldown-cmark` |
| **`cli`** | `rmt` 실행 바이너리 | `clap`, `rayon` — 라이브러리 소비자에겐 새지 않는 opt-in |
| **`python`** | PyO3 cdylib 바인딩 | `pyo3`(abi3) |

```toml
# default = ["docx", "pptx", "xlsx", "hwpx", "pdf", "html", "markdown"]   ← 전 포맷, zero FFI

# 최소 구성 예: DOCX + HTML + Markdown 만
rust_markdown_transformer = { version = "0.1", default-features = false, features = ["docx", "html", "markdown"] }
```

> **default 빌드는 외부 .so/.dll 및 subprocess 를 요구하지 않는다.** 어떤 백엔드에도 안심하고 정적 링크할 수 있다.

---

## 4. 아키텍처

```text
구체 파서(feature) → FormatParser(트레이트) → IR(Document) → 두 갈래
    갈래 1 → MarkdownRenderer → Markdown 문자열
    갈래 2 → SemanticChunker  → Vec<Chunk> → 벡터 DB Loader
```

핵심은 **공통 IR 레이어**다. 각 파서는 포맷 고유 구조를 IR 로 변환하고, 렌더러와 청커는 오직 IR 만 본다.
→ 파서와 렌더러/청커가 완전히 분리 → 새 포맷 추가가 O(1)에 가깝다.

- **입력** — 구체 파서가 [`FormatParser`](#61-formatparser-트레이트) 구현체로 [`ParserRegistry`](#62-parserregistry) 에 등록된다.
- **디스패치** — 레지스트리가 **확장자 우선, 매직바이트 폴백**으로 파서를 고른다.
- **변환** — 파서가 [`Document`](#5-공통-ir-레퍼런스) IR 을 만든다.
- **출력** — [`MarkdownRenderer`](#63-markdownrenderer) 가 Markdown 을, [`SemanticChunker`](#8-semantic-chunking) 가 청크를 파생한다.

---

## 5. 공통 IR 레퍼런스

`ir` 모듈. 모든 타입은 `serde::{Serialize, Deserialize}` 를 구현하므로 그대로 `*.ir.json` 으로 떨어뜨릴 수 있다.

```rust
pub struct Document {
    pub metadata: DocumentMetadata,
    pub blocks:   Vec<Block>,
}

pub struct DocumentMetadata {
    pub title:             Option<String>,
    pub author:            Option<String>,
    pub created_at:        Option<chrono::DateTime<chrono::Utc>>,
    pub source_format:     SourceFormat,
    pub original_filename: String,
    pub page_count:        Option<usize>,
    pub language:          Option<String>,
}

/// serde 직렬화 시 소문자(예: "docx").
pub enum SourceFormat {
    Docx, Pptx, Xlsx, Hwp, Hwpx, Pdf, Html, Markdown, Epub, Rtf, Odt, Unknown,
}
```

### 블록 레벨 — `Block`

```rust
pub enum Block {
    Heading       { level: u8, text: String },        // h1~h6
    Paragraph     (Vec<Inline>),
    Table         (Table),
    List          { ordered: bool, items: Vec<ListItem> },
    CodeBlock     { lang: Option<String>, code: String },
    Quote         (Vec<Inline>),
    HorizontalRule,
    Image         { alt: String, data: ImageData },
    Math          { latex: String, display: bool },   // 인라인/디스플레이 수식
    PageBreak,                                         // PPT 슬라이드 / PDF 페이지 경계
    Footnote      { id: String, content: Vec<Inline> },
}
```

### 인라인 레벨 — `Inline`

```rust
pub enum Inline {
    Text   (String),
    Bold   (Box<Inline>),
    Italic (Box<Inline>),
    Strike (Box<Inline>),
    Code   (String),
    Link   { text: String, url: String },
    LineBreak,
}
```

### 보조 타입

```rust
pub struct ListItem {
    pub content: Vec<Inline>,
    pub sublist: Option<Box<NestedList>>,   // 중첩 목록
}

pub struct NestedList { pub ordered: bool, pub items: Vec<ListItem> }

pub struct Table {
    pub headers: Vec<String>,
    pub rows:    Vec<Vec<String>>,
    pub caption: Option<String>,
}

pub enum ImageData {
    Base64 { mime: String, data: String },  // data: URI
    Path   (String),                         // 로컬/상대 경로
    Url    (String),                         // 외부 URL
}
```

생성 헬퍼: `Document::new(meta)` / `Document::push(block)` / `DocumentMetadata::new(fmt, filename)` /
`Inline::text("...")` / `ListItem::new(content)`.

---

## 6. 공개 API 레퍼런스

### 6.1 `FormatParser` 트레이트

```rust
pub trait FormatParser: Send + Sync {
    fn supported_extensions(&self) -> &[&str];                 // 예: &["docx"]
    fn can_parse_bytes(&self, header: &[u8]) -> bool;          // 매직바이트 식별
    fn name(&self) -> &'static str;                            // 로그/디버깅용
    fn parse(&self, input: &mut dyn Read, filename: &str)
        -> Result<Document, ParseError>;
}
```

### 6.2 `ParserRegistry`

```rust
ParserRegistry::with_defaults() -> Self          // 활성 feature 의 기본 파서 전부 등록
ParserRegistry::empty()         -> Self
fn register(&mut self, parser: Box<dyn FormatParser>)
fn parser_names(&self) -> Vec<&'static str>
fn is_supported(&self, path: &Path) -> bool

fn parse_to_ir(&self, path: &Path)        -> Result<Document, ConvertError>
fn convert_to_markdown(&self, path: &Path) -> Result<String,  ConvertError>
fn parse_reader(&self, reader: &mut dyn Read, filename: &str, ext_hint: Option<&str>)
                                           -> Result<Document, ConvertError>
```

- **디스패치 규칙**: 확장자로 먼저 찾고, 없으면 매직바이트(`can_parse_bytes`)로 폴백한다.
- `parse_reader` 는 reader 전체를 메모리로 읽어 매직바이트 판별 + Seek 가능 커서로 파서에 넘긴다.
  stdin 파이프처럼 확장자를 모를 때 `ext_hint`(예: `Some("pdf")`)를 준다.

### 6.3 `MarkdownRenderer`

```rust
MarkdownRenderer::render(doc: &Document) -> String        // frontmatter + 본문
MarkdownRenderer::render_blocks(blocks: &[Block]) -> String // 본문만
```

`render` 산출물 선두에 YAML frontmatter 가 붙는다(벡터 DB 메타데이터로 그대로 사용 가능):

```yaml
---
title: 분기 보고서
author: ""
source_format: hwpx
original_filename: report.hwpx
page_count: 12        # Some 일 때만
language: ko          # Some 일 때만
created_at: 2026-...  # Some 일 때만 (RFC 3339)
---
```

표 셀의 `|`·개행 이스케이프, 헤딩/셀 공백 접기, 중첩 목록 들여쓰기, 코드블록 백틱 충돌 시 긴 펜스 선택을
모두 결정적으로 처리한다.

### 6.4 에러 타입

```rust
pub enum ParseError {                  // 개별 파서가 IR 변환 중 내는 에러
    Io(std::io::Error),
    Container { format, detail },       // zip/OLE2 등 컨테이너 손상·엔트리 누락
    Markup    { format, detail },       // XML/마크업 파싱 실패
    Encoding  { format, detail },       // 인코딩 디코딩 실패
    Unsupported { format, detail },
}

pub enum ConvertError {                // 레지스트리 상위 API 에러
    Io(std::io::Error),
    UnsupportedFormat(String),          // 등록된 파서 없음
    Parse(ParseError),
}
```

> 에러 타입은 **optional 의존성에 의존하지 않는다.** `zip`/`quick-xml`/`calamine` 등의 구체 에러는 각 파서에서
> 문자열로 흡수하므로, 어떤 feature 조합에서도 항상 컴파일된다.

---

## 7. 지원 포맷별 동작과 한계

| 포맷 | 확장자 | 구현 엔진 | 추출 항목 |
|---|---|---|---|
| **DOCX** | `docx` | `zip`+`quick-xml` | 헤딩(styles.xml 매핑), 단락, **굵게/기울임/취소선**, 표, 목록, **이미지(base64 data URI)**, 제목/작성자(core.xml) |
| **PPTX** | `pptx` | `zip`+`quick-xml` | 슬라이드별 제목→h2, 본문 단락, **굵게/기울임**, **표(DrawingML), 이미지**, 슬라이드 경계→PageBreak, 슬라이드 수 |
| **XLSX** | `xlsx` `xlsm` | `calamine` | 시트별 제목→h2, 사용영역→표, 빈 행/열 trim |
| **HWPX** | `hwpx` | `zip`+`quick-xml` | 헤딩(header.xml `Outline N`), 단락, 표, **이미지(BinData)**, 제목(content.hpf) |
| **PDF** | `pdf` | `pdf-extract`+`lopdf` | 본문 텍스트(**한글 CID/ToUnicode 포함**), **폰트크기 기반 헤딩**, **XY-Cut 읽기순서(다단 분리)**, **표 복원(좌표 군집 Stream 방식)**, **내장 이미지(JPEG/JP2)**, 단락, 제목/작성자/페이지 수 |
| **HTML** | `html` `htm` `xhtml` | `scraper` | `<article>/<main>/<body>` 우선, 헤딩/단락/목록(중첩)/표/코드/인용/이미지/링크/강조 |
| **Markdown** | `md` `markdown` `mdown` `mkd` | `pulldown-cmark` | 헤딩/단락/목록/표/코드/인용/링크/이미지/강조 **재정규화** |

공통:
- 입력 선두 **UTF-8 BOM 은 자동 제거**된다.
- 표 **병합셀(rowspan/colspan)** 은 v0.1 범위에서 미지원(첫 셀 값 보존).
- **임베디드 이미지**는 `Block::Image` 로 추출되어 Markdown `![alt](data:...)` 로 렌더된다. OOXML(docx/pptx)·HWPX 는 원본 바이트를 base64 로, PDF 는 JPEG/JP2 스트림을 그대로 담는다.
- **PDF 표 복원**은 글자 좌표 정렬 휴리스틱(Stream 방식)이라 **명확한 격자에 한해** 동작하고, 열 정렬이 어긋나면 표로 보지 않고 본문 단락으로 폴백한다(오탐 억제 우선).
- 손상 입력에 대해 PDF 파서는 panic 을 격리해 `ParseError` 로 변환한다.

---

## 8. Semantic Chunking

Markdown 을 N-token 으로 무지성 분할하지 않고, **IR 의 Heading 경계를 1급 분할점**으로 사용한 뒤
`max_tokens` 초과분만 블록 경계로 추가 분할한다. 모든 청크에 **조상 헤딩 경로**(`heading_path`)를 부여해
계층 검색·citation 품질을 끌어올린다.

```rust
pub struct SemanticChunker {
    pub max_tokens:     usize,   // 예: 512
    pub overlap_tokens: usize,   // 예: 64 (인접 청크 겹침 → recall 향상)
    pub heading_levels: Vec<u8>, // 어떤 레벨에서 자를지 (예: [1, 2])
}
impl Default for SemanticChunker { /* 512 / 64 / [1,2] */ }

pub struct Chunk {
    pub heading_path: Vec<String>,   // ["1장", "1.2 절"]
    pub content:      String,        // Markdown
    pub token_count:  usize,
    pub metadata:     DocumentMetadata,
}

chunker.chunk(&doc)                          // 기본 토큰 카운터
chunker.chunk_with(&doc, &my_token_counter)  // 임의 카운터 주입
```

### 토큰 카운팅

`TokenCounter` 트레이트로 추상화돼 있다. 기본값은 **의존성 0 다국어 근사** [`HeuristicTokenCounter`]:

- 라틴/ASCII: 약 4글자 ≈ 1토큰
- CJK(한·중·일): 글자당 ≈ 1토큰

```rust
pub trait TokenCounter { fn count(&self, text: &str) -> usize; }
```

정확한 값이 필요하면 `tiktoken-rs`/HuggingFace `tokenizers` 를 감싼 `TokenCounter` 를 직접 구현해
`chunk_with` 에 주입하면 된다(코어 변경 불필요).

---

## 9. CLI 도구 (`rmt`)

`--features cli` 로 빌드된다.

```bash
cargo build --release --features cli
```

```bash
# 단일 파일 변환 (출력 생략 시 stdout)
rmt convert ./report.docx -o ./report.md

# 디렉토리 일괄 변환 (재귀, 하위 폴더 구조 보존, 병렬)
rmt batch ./docs/ -o ./out/ --parallel 8

# IR 파싱 → Semantic Chunking → JSONL
rmt chunk ./report.pdf --max-tokens 512 --overlap 64 --heading-levels 1,2 -o ./report.jsonl

# stdin/stdout 파이프 (포맷 힌트 필요)
cat input.pdf | rmt convert --from pdf > output.md
```

| 서브커맨드 | 인자 | 동작 |
|---|---|---|
| `convert` | `[input]` `-o/--output` `--from <ext>` | 단일 파일/stdin → Markdown. 출력 생략 시 stdout. |
| `batch` | `<input_dir>` `-o/--output <dir>` `--parallel <N>` | 디렉토리 재귀 순회, 지원 파일 전부 변환. `N=0` 이면 자동. `rayon` 병렬. |
| `chunk` | `<input>` `--max-tokens` `--overlap` `--heading-levels` `-o` | 청킹 결과를 JSONL(청크당 한 줄)로 출력. |

---

## 10. Python 바인딩 (PyO3)

대부분의 RAG / 벡터 DB 백엔드가 Python 이므로, 이 라이브러리를 ingestion 단에 한 줄로 꽂아
포맷 다양성 문제를 일괄 해결한다. **abi3(stable ABI)** 로 빌드되어 Python 버전 변화에 forward-compatible 하다.

### 설치 (Python 사용자)

```bash
# PyPI 게시 후 — Rust 툴체인 불필요, 휠을 그대로 받는다
pip install rust_markdown_transformer

# PyPI 게시 전(또는 최신 main 을 쓰고 싶을 때) — GitHub 소스에서 설치
# 이 경로는 설치 머신에 Rust 툴체인이 필요하다(소스를 직접 컴파일).
pip install "git+https://github.com/arabangoo/rust_markdown_transformer"
```

설치하면 어떤 포맷이든 한 줄로 Markdown 으로 정규화할 수 있다.

```python
import rust_markdown_transformer as rmt

md     = rmt.convert_to_markdown("report.hwpx")           # Markdown 문자열 (청킹·임베딩용)
ir     = rmt.convert_to_ir_json("report.pdf")             # IR JSON 문자열 (멀티모달/citation 안전망)
chunks = rmt.convert_to_chunks("report.docx", 512, 64)    # 청크 목록 JSON (벡터 DB 적재용)
ok     = rmt.is_supported("a.xlsx")                       # 지원 여부 (True/False)
```

### 빌드 (개발자 · 게시자)

루트의 `pyproject.toml`(maturin 백엔드)이 빌드 메타데이터를 제공한다. abi3 휠이라 Python 3.9+ 단일 휠로 호환된다.
`[tool.maturin] features = ["python"]` 덕분에 `--features python` 을 생략해도 된다.

```bash
# (a) 개발용 — 현재 venv 에 즉시 설치
pip install maturin
maturin develop --release

# (b) 배포 휠 빌드
maturin build --release             # target/wheels/rust_markdown_transformer-*.whl
pip install target/wheels/rust_markdown_transformer-*.whl

# (c) GitHub 소스에서 바로 설치 (설치 머신에 Rust 툴체인 필요)
pip install "git+https://github.com/arabangoo/rust_markdown_transformer"
```

### API

```python
import rust_markdown_transformer as rmt

rmt.__version__                               # "0.1.0"
rmt.supported_parsers()                       # ['docx', 'pptx', 'xlsx', 'hwpx', 'pdf', 'html', 'markdown']
rmt.is_supported("a.docx")                    # True

md   = rmt.convert_to_markdown("report.hwpx") # 청킹·임베딩용 Markdown 문자열
ir   = rmt.convert_to_ir_json("report.hwpx")  # IR JSON 문자열 (멀티모달/citation 안전망)
js   = rmt.convert_to_chunks("report.pdf",    # 청크 목록의 JSON 문자열
                             max_tokens=512, overlap=64, heading_levels=[1, 2])
```

### 파이프라인 통합 예시

```python
# LangChain
from langchain.text_splitter import MarkdownHeaderTextSplitter
import rust_markdown_transformer as rmt
md = rmt.convert_to_markdown("./contract.docx")          # 포맷 무관 변환
docs = MarkdownHeaderTextSplitter(headers_to_split_on=[("#","h1"),("##","h2")]).split_text(md)

# 자체 파이프라인 — 청킹까지 한 번에
import json
for path in Path("./corpus").rglob("*"):
    if rmt.is_supported(str(path)):
        chunks = json.loads(rmt.convert_to_chunks(str(path), 512, 64))
        qdrant.upsert(collection="kb", points=embed(chunks))
```

---

## 11. 서비스 파이프라인에 붙이기 (통합 레시피)

이 라이브러리는 단독 실행 앱이 아니라 **당신의 ingestion 파이프라인에 박아 넣는 코어 의존성**이다.
"포맷별 로더 분기"를 입력 단 한 곳으로 흡수하는 것이 핵심 가치다. 호스트 환경에 따라 아래 표면 중 하나를 고른다.

| 호스트 | 표면 | 설치 |
|---|---|---|
| Python RAG (LangChain/LlamaIndex/자체) | Python 모듈 | `pip install "git+https://github.com/arabangoo/rust_markdown_transformer"` |
| Rust 서비스 | crate | `Cargo.toml` 에 git 의존성 |
| 타 언어 / 셸 / 배치 / 오케스트레이션 | CLI(`rmt`) | `cargo install --git https://github.com/arabangoo/rust_markdown_transformer rust_markdown_transformer --features cli` |

### 11.1 Python RAG 파이프라인 — 포맷 분기 제거

기존에 포맷마다 다른 로더로 분기하던 코드를 **입력 단 한 줄**로 대체한다.

```python
# Before — 포맷마다 별도 로더 (python-docx / pdfminer / pyhwp / BeautifulSoup ...)
# After — 포맷 무관 단일 진입점
import rust_markdown_transformer as rmt
md = rmt.convert_to_markdown(path)   # docx/pptx/xlsx/hwpx/pdf/html/md 전부
```

**LangChain** — `MarkdownHeaderTextSplitter` 와 직결:

```python
from langchain.text_splitter import MarkdownHeaderTextSplitter
import rust_markdown_transformer as rmt

md = rmt.convert_to_markdown("./contract.hwpx")
splitter = MarkdownHeaderTextSplitter(headers_to_split_on=[("#", "h1"), ("##", "h2")])
docs = splitter.split_text(md)        # 이후 embedding/indexing 은 그대로
```

**LlamaIndex** — `MarkdownNodeParser` 와 직결:

```python
from llama_index.core import Document
from llama_index.core.node_parser import MarkdownNodeParser
import rust_markdown_transformer as rmt

md = rmt.convert_to_markdown("./report.pdf")
nodes = MarkdownNodeParser().get_nodes_from_documents([Document(text=md)])
```

**자체 인제스트 워커** — 코퍼스 순회 → 청킹 → 임베딩 → 벡터 DB. 미지원 파일 skip, 파일 해시로 idempotent 재인제스트:

```python
import hashlib, json
from pathlib import Path
import rust_markdown_transformer as rmt

def ingest(corpus: str, collection):
    for path in Path(corpus).rglob("*"):
        if not path.is_file() or not rmt.is_supported(str(path)):
            continue
        try:
            chunks = json.loads(rmt.convert_to_chunks(str(path), max_tokens=512, overlap=64))
        except RuntimeError as e:           # 손상 파일 등은 건너뛰고 계속
            print(f"skip {path}: {e}")
            continue
        points = []
        for i, c in enumerate(chunks):
            doc_id = hashlib.sha1(f"{path}:{i}".encode()).hexdigest()  # 재실행 시 upsert 멱등
            points.append({
                "id": doc_id,
                "vector": embed(c["content"]),
                "payload": {
                    "text": c["content"],
                    "heading_path": c["heading_path"],   # 계층 검색·citation 메타
                    "source": str(path),
                    "source_format": c["metadata"]["source_format"],
                },
            })
        collection.upsert(points=points)
```

**Dual-track 저장** — 검색용 Markdown + 안전망 IR JSON 동시 적재 (멀티모달/정밀 citation 대비):

```python
md = rmt.convert_to_markdown(path)        # 임베딩·검색용
ir = rmt.convert_to_ir_json(path)         # 병합셀 표/원본 구조 보존 — object storage 등에 보관
vector_db.upsert(chunks=split(md), metadata={"ir_ref": store_blob(ir)})
```

### 11.2 Rust 서비스에 임베드

```toml
[dependencies]
rust_markdown_transformer = { git = "https://github.com/arabangoo/rust_markdown_transformer", tag = "v0.1.0" }
```

파싱은 동기·CPU 바운드이므로, async 서버(axum/actix)에서는 `spawn_blocking` 으로 감싼다.
업로드 바이트는 [`parse_reader`](#62-parserregistry) 로 직접 처리(파일 저장 불필요):

```rust
use std::io::Cursor;
use rust_markdown_transformer::{MarkdownRenderer, ParserRegistry};

// 예: axum 핸들러 — 업로드된 문서 바이트 → Markdown
async fn convert_handler(filename: String, bytes: Vec<u8>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let registry = ParserRegistry::with_defaults();
        let ext = std::path::Path::new(&filename)
            .extension().and_then(|e| e.to_str());
        let doc = registry
            .parse_reader(&mut Cursor::new(bytes), &filename, ext)
            .map_err(|e| e.to_string())?;
        Ok(MarkdownRenderer::render(&doc))
    })
    .await
    .map_err(|e| e.to_string())?
}
```

다수 파일 배치는 `rayon` 으로 병렬화하면 된다(소비자 측에서 `par_iter`).

### 11.3 타 언어 / 셸 / 오케스트레이션 — CLI 래핑

Python·Rust 가 아닌 스택(Java/Node/Go 등)이나 배치 잡에서는 `rmt` 바이너리를 subprocess 로 호출한다.

```bash
# 컨테이너/배치: 디렉토리 통째로 변환
rmt batch /data/raw -o /data/markdown --parallel 8

# 파이프라인 스테이지: 청킹 JSONL 을 다음 단계로 스트리밍
rmt chunk /data/raw/report.pdf --max-tokens 512 -o - | my-embedder --stdin
```

```python
# 예: Airflow / cron 등에서 subprocess 호출 (언어 무관 통합)
import subprocess
subprocess.run(["rmt", "batch", "./raw", "-o", "./md", "--parallel", "8"], check=True)
```

> 단일 정적 바이너리라 컨테이너 이미지에 `rmt` 하나만 넣으면 된다 — JVM/Node/Python 런타임 의존이 없다.

---

## 12. 새 포맷 파서 추가하기

코어를 전혀 건드리지 않고 서드파티 crate 에서도 파서를 끼울 수 있다.

```rust
use rust_markdown_transformer::{
    Document, DocumentMetadata, SourceFormat, Block, Inline, FormatParser, ParserRegistry,
};
use rust_markdown_transformer::error::ParseError;
use std::io::Read;

struct PlainTextParser;

impl FormatParser for PlainTextParser {
    fn supported_extensions(&self) -> &[&str] { &["txt", "log"] }
    fn name(&self) -> &'static str { "plaintext" }
    fn can_parse_bytes(&self, _h: &[u8]) -> bool { false }   // 확장자 디스패치만

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let mut s = String::new();
        input.read_to_string(&mut s)?;
        let mut doc = Document::new(DocumentMetadata::new(SourceFormat::Unknown, filename));
        for para in s.split("\n\n") {
            if !para.trim().is_empty() {
                doc.push(Block::Paragraph(vec![Inline::text(para.trim())]));
            }
        }
        Ok(doc)
    }
}

let mut registry = ParserRegistry::with_defaults();
registry.register(Box::new(PlainTextParser));
let md = registry.convert_to_markdown("notes.txt".as_ref())?;
```

OOXML 계열(ZIP+XML)이라면 `parsers::ooxml::OoxmlPackage` 헬퍼로 zip 을 풀어 XML 파트만 꺼내쓸 수 있다.

---

## 13. 빌드 · Feature 조합 · 테스트

### 레포를 clone 해서 직접 빌드해 쓰기

이 저장소를 clone 한 경우, 쓰려면 **Rust 툴체인(stable, 1.74 이상 권장)** 이 필요하고 한 번 빌드해야 한다. Rust 는 컴파일 언어라 소스만으로는 import/실행되지 않는다. 용도에 따라 셋 중 하나를 고른다.

| 쓰는 방식 | 빌드 명령 | 결과물 |
|---|---|---|
| CLI 도구 | `cargo build --release --features cli` | `target/release/rmt` 단일 바이너리 ([9장](#9-cli-도구-rmt)) |
| Python 모듈 | `pip install maturin && maturin develop --release` | 현재 venv 에 `import rust_markdown_transformer` 설치 ([10장](#10-python-바인딩-pyo3)) |
| Rust 라이브러리 | `Cargo.toml` 에 `path`/`git` 의존성으로 추가 | 다른 Rust 프로젝트에 링크 ([3장](#3-설치와-cargo-feature)) |

> Python 빌드는 Rust 툴체인 + `maturin` + Python 헤더가 필요하다. 최종 사용자에게 Rust 없이 배포하려면 휠을 빌드해 PyPI 등에 게시하면 된다(빌드 부담은 게시자에게만, 사용자는 `pip install` 만).

### Feature 조합 빌드 · 테스트

```bash
# 기본: 전 포맷 + zero FFI (정적 단일 바이너리)
cargo build --release

# 최소 구성
cargo build --release --no-default-features --features docx,html,markdown

# CLI 바이너리
cargo build --release --features cli

# Python cdylib
cargo build --release --features python      # 또는 maturin develop --features python

# 테스트 / 린트
cargo test
cargo clippy --all-targets
cargo run --example convert -- ./some.docx   # 단일 파일 변환 예제
```

테스트는 외부 파일 의존 없이 **OOXML/HWPX zip 과 PDF 를 테스트 안에서 합성**해 각 파서·렌더러·청커·
레지스트리 디스패치를 결정적으로 검증한다(`tests/integration.rs`).

---

## 14. 디렉토리 구조

```text
rust_markdown_transformer/
  Cargo.toml
  README.md              # 이 문서
  LICENSE                # Apache-2.0
  src/
    lib.rs               # 크레이트 루트 · re-export
    ir.rs                # 공통 IR (Document/Block/Inline/Table/…)
    error.rs             # ParseError / ConvertError
    registry.rs          # FormatParser 트레이트 + ParserRegistry
    renderer.rs          # IR → Markdown
    chunker.rs           # SemanticChunker / TokenCounter
    python.rs            # PyO3 바인딩 (feature = "python")
    bin/
      rmt.rs             # CLI 바이너리 (feature = "cli")
    parsers/
      mod.rs             # feature 게이트 + re-export
      ooxml.rs           # OOXML/OWPML 공통 zip 언패커 (관계 .rels · 이미지 해석 포함)
      media.rs           # 임베디드 이미지 공통 헬퍼 (base64 · MIME 추정)
      pdf_layout.rs      # PDF 좌표 기반 레이아웃 (헤딩 · 읽기순서 · 표 복원)
      docx.rs  pptx.rs  xlsx.rs  hwpx.rs  pdf.rs  html.rs  markdown.rs
  examples/
    convert.rs           # 단일 파일 변환 예제
  tests/
    integration.rs       # 합성 픽스처 기반 통합 테스트
```

---

## 15. 라이선스

Apache-2.0
