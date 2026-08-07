# rust_markdown_transformer

> **A universal document-to-Markdown conversion library, written in Rust.**
>
> Converts `docx · pptx · xlsx · hwpx · pdf · html · markdown` documents into
> **vector-DB / RAG-friendly Markdown**, deterministically and fast.

This document is the **complete developer manual** for the library. It covers the design philosophy, the public API, per-format behavior and limitations, CLI and Python usage, how to add a new format, and the build and test workflow.

**Key references**

1. XY-Cut++: Advanced Layout Ordering via Hierarchical Mask Mechanism on a Novel Benchmark - https://arxiv.org/abs/2504.10258
2. LayoutReader: Pre-training of Text and Layout for Reading Order Detection - https://arxiv.org/abs/2108.11591
3. Nougat: Neural Optical Understanding for Academic Documents - https://arxiv.org/abs/2308.13418

---

## Table of Contents

1. [Key Features](#1-key-features)
2. [Quick Start](#2-quick-start)
3. [Installation and Cargo Features](#3-installation-and-cargo-features)
4. [Architecture](#4-architecture)
5. [Common IR Reference](#5-common-ir-reference)
6. [Public API Reference](#6-public-api-reference)
7. [Per-Format Behavior and Limitations](#7-per-format-behavior-and-limitations)
8. [Semantic Chunking](#8-semantic-chunking)
9. [CLI Tool (`rmt`)](#9-cli-tool-rmt)
10. [Python Bindings (PyO3)](#10-python-bindings-pyo3)
11. [Embedding into a Service Pipeline (Integration Recipes)](#11-embedding-into-a-service-pipeline-integration-recipes)
12. [Adding a New Format Parser](#12-adding-a-new-format-parser)
13. [Build, Feature Combinations, and Testing](#13-build-feature-combinations-and-testing)
14. [Directory Layout](#14-directory-layout)
15. [License](#15-license)

---

## 1. Key Features

The most underrated part of a RAG / vector-DB pipeline is **document ingestion**. No matter how good the model is, poor input processing breaks retrieval and answers. This library is not a plain text extractor; it aims to be a **structure-preserving conversion engine that maximizes indexing quality**.

| Principle | What it means |
|---|---|
| **Deterministic** | Same input always produces the same output. Easy to cache, test, and debug. This is why ML-based tools were ruled out as the first choice. |
| **Structure-preserving** | Not a flat text dump. It reproduces **heading hierarchy, tables, lists, code blocks, links, and emphasis** as Markdown syntax. |
| **Plugin-extensible** | A new format needs only one trait, [`FormatParser`](#61-the-formatparser-trait). The core stays untouched. |
| **Zero-dependency, self-contained** | The default build is **pure Rust, zero FFI, zero subprocess**. Add one line to `Cargo.toml` and drop it in with confidence. No npm, JVM, or Python runtime required. |

### Why Markdown-first

Markdown is the de facto standard for vector-DB chunking.

- **Headings (`#`, `##`) are a universal marker for semantic boundaries.** Most chunkers, such as LangChain's `MarkdownHeaderTextSplitter` and LlamaIndex's `MarkdownNodeParser`, treat headings as first-class citizens.
- **It is the text format LLMs understand best.** When retrieved chunks are injected into context, answer quality is consistently better than with HTML, XML, or raw text.
- **Token-efficient and easy to debug.** You can open the `.md` file directly and see exactly what the embedding model saw.

Markdown is lossy (merged cells, PDF coordinates, and the visual meaning of images are lost). To compensate, the same [IR](#5-common-ir-reference) drives two tracks at once: **Markdown (the primary output) plus IR JSON (a safety net)**.

```text
source (any format) -> [Rust parser] -> IR -> two tracks
    track 1 -> Markdown (.md)      -> vector DB / RAG (99% of cases)
    track 2 -> IR JSON (.ir.json)  -> multimodal RAG / precise citation / lossless reprocessing
```

---

## 2. Quick Start

### Rust library

```rust
use rust_markdown_transformer::{ParserRegistry, SemanticChunker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = ParserRegistry::with_defaults();

    // 1) Simple conversion: the parser is auto-selected by extension / magic bytes
    let md = registry.convert_to_markdown("report.docx".as_ref())?;
    std::fs::write("report.md", md)?;

    // 2) Chunking for vector-DB ingestion
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
md = rmt.convert_to_markdown("report.hwpx")   # normalize any format in one line
```

---

## 3. Installation and Cargo Features

`Cargo.toml`:

```toml
[dependencies]
rust_markdown_transformer = "0.1"
```

### Feature list

| Feature | Enables | Notes |
|---|---|---|
| `docx` | DOCX parser | `zip`, `quick-xml` |
| `pptx` | PPTX parser | `zip`, `quick-xml` |
| `xlsx` | XLSX/XLSM parser | `calamine` |
| `hwpx` | Hancom HWPX (OWPML) parser | `zip`, `quick-xml` |
| `pdf` | PDF parser | `pdf-extract` (text), `lopdf` (metadata) |
| `html` | HTML parser | `scraper` |
| `markdown` | Markdown re-normalization | `pulldown-cmark` |
| **`cli`** | the `rmt` binary | `clap`, `rayon`. An opt-in that does not leak to library consumers. |
| **`python`** | PyO3 cdylib bindings | `pyo3` (abi3) |

```toml
# default = ["docx", "pptx", "xlsx", "hwpx", "pdf", "html", "markdown"]  # all formats, zero FFI

# Minimal example: DOCX + HTML + Markdown only
rust_markdown_transformer = { version = "0.1", default-features = false, features = ["docx", "html", "markdown"] }
```

> **The default build requires no external .so/.dll and no subprocess.** You can statically link it into any backend with confidence.

---

## 4. Architecture

```text
concrete parser (feature) -> FormatParser (trait) -> IR (Document) -> two paths
    path 1 -> MarkdownRenderer -> Markdown string
    path 2 -> SemanticChunker  -> Vec<Chunk> -> vector-DB loader
```

The heart of it is the **common IR layer**. Each parser converts a format's own structure into IR, and the renderer and chunker see only the IR. Parsers are therefore fully decoupled from the renderer and chunker, which makes adding a new format close to O(1).

- **Input**: a concrete parser is registered in the [`ParserRegistry`](#62-parserregistry) as a [`FormatParser`](#61-the-formatparser-trait) implementation.
- **Dispatch**: the registry picks a parser by **extension first, with a magic-byte fallback**.
- **Conversion**: the parser produces a [`Document`](#5-common-ir-reference) IR.
- **Output**: [`MarkdownRenderer`](#63-markdownrenderer) derives Markdown, and [`SemanticChunker`](#8-semantic-chunking) derives chunks.

---

## 5. Common IR Reference

The `ir` module. Every type implements `serde::{Serialize, Deserialize}`, so it can be dumped directly to `*.ir.json`.

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

/// Serialized in lowercase by serde (e.g. "docx").
pub enum SourceFormat {
    Docx, Pptx, Xlsx, Hwp, Hwpx, Pdf, Html, Markdown, Epub, Rtf, Odt, Unknown,
}
```

### Block level: `Block`

```rust
pub enum Block {
    Heading       { level: u8, text: String },        // h1 to h6
    Paragraph     (Vec<Inline>),
    Table         (Table),
    List          { ordered: bool, items: Vec<ListItem> },
    CodeBlock     { lang: Option<String>, code: String },
    Quote         (Vec<Inline>),
    HorizontalRule,
    Image         { alt: String, data: ImageData },
    Math          { latex: String, display: bool },   // inline / display math
    PageBreak,                                         // PPT slide / PDF page boundary
    Footnote      { id: String, content: Vec<Inline> },
}
```

### Inline level: `Inline`

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

### Supporting types

```rust
pub struct ListItem {
    pub content: Vec<Inline>,
    pub sublist: Option<Box<NestedList>>,   // nested list
}

pub struct NestedList { pub ordered: bool, pub items: Vec<ListItem> }

pub struct Table {
    pub headers: Vec<String>,
    pub rows:    Vec<Vec<String>>,
    pub caption: Option<String>,
}

pub enum ImageData {
    Base64 { mime: String, data: String },  // data: URI
    Path   (String),                         // local / relative path
    Url    (String),                         // external URL
}
```

Construction helpers: `Document::new(meta)` / `Document::push(block)` / `DocumentMetadata::new(fmt, filename)` /
`Inline::text("...")` / `ListItem::new(content)`.

---

## 6. Public API Reference

### 6.1 The `FormatParser` Trait

```rust
pub trait FormatParser: Send + Sync {
    fn supported_extensions(&self) -> &[&str];                 // e.g. &["docx"]
    fn can_parse_bytes(&self, header: &[u8]) -> bool;          // magic-byte identification
    fn name(&self) -> &'static str;                            // for logging / debugging
    fn parse(&self, input: &mut dyn Read, filename: &str)
        -> Result<Document, ParseError>;
}
```

### 6.2 `ParserRegistry`

```rust
ParserRegistry::with_defaults() -> Self          // registers every default parser for the enabled features
ParserRegistry::empty()         -> Self
fn register(&mut self, parser: Box<dyn FormatParser>)
fn parser_names(&self) -> Vec<&'static str>
fn is_supported(&self, path: &Path) -> bool

fn parse_to_ir(&self, path: &Path)         -> Result<Document, ConvertError>
fn convert_to_markdown(&self, path: &Path) -> Result<String,  ConvertError>
fn parse_reader(&self, reader: &mut dyn Read, filename: &str, ext_hint: Option<&str>)
                                           -> Result<Document, ConvertError>
```

- **Dispatch rule**: try the extension first, then fall back to magic bytes (`can_parse_bytes`).
- `parse_reader` reads the entire reader into memory to run magic-byte detection and then hands a seekable cursor to the parser. When the extension is unknown (for example, a stdin pipe), pass an `ext_hint` such as `Some("pdf")`.

### 6.3 `MarkdownRenderer`

```rust
MarkdownRenderer::render(doc: &Document) -> String          // frontmatter + body
MarkdownRenderer::render_blocks(blocks: &[Block]) -> String // body only
```

`render` prepends YAML frontmatter to the output, usable directly as vector-DB metadata:

```yaml
---
title: Quarterly Report
author: ""
source_format: hwpx
original_filename: report.hwpx
page_count: 12        # only when Some
language: ko          # only when Some
created_at: 2026-...  # only when Some (RFC 3339)
---
```

Escaping of `|` and newlines in table cells, collapsing of whitespace in headings and cells, indentation of nested lists, and choosing a longer fence when code-block backticks collide are all handled deterministically.

### 6.4 Error Types

```rust
pub enum ParseError {                  // errors raised by an individual parser during IR conversion
    Io(std::io::Error),
    Container { format, detail },       // container corruption or missing entry (zip, OLE2, etc.)
    Markup    { format, detail },       // XML / markup parse failure
    Encoding  { format, detail },       // encoding / decoding failure
    Unsupported { format, detail },
}

pub enum ConvertError {                // top-level registry API errors
    Io(std::io::Error),
    UnsupportedFormat(String),          // no registered parser
    Parse(ParseError),
}
```

> The error types **do not depend on optional dependencies.** Concrete errors from `zip`, `quick-xml`, `calamine`, and so on are absorbed into strings inside each parser, so the crate always compiles under any feature combination.

---

## 7. Per-Format Behavior and Limitations

| Format | Extensions | Engine | Extracted |
|---|---|---|---|
| **DOCX** | `docx` | `zip` + `quick-xml` | headings (mapped from styles.xml), paragraphs, **bold/italic/strikethrough**, tables, lists, **images (base64 data URI)**, title/author (core.xml) |
| **PPTX** | `pptx` | `zip` + `quick-xml` | per-slide title to h2, body paragraphs, **bold/italic**, **tables (DrawingML), images**, slide boundary to PageBreak, slide count |
| **XLSX** | `xlsx` `xlsm` | `calamine` | per-sheet title to h2, used range to table, trimming of empty rows/columns |
| **HWPX** | `hwpx` | `zip` + `quick-xml` | headings (header.xml `Outline N`), paragraphs, tables, **images (BinData)**, title (content.hpf) |
| **PDF** | `pdf` | `pdf-extract` + `lopdf` | body text (**including Korean CID / ToUnicode**), **font-size-based headings**, **XY-Cut reading order (multi-column separation)**, **table reconstruction (coordinate-clustering, stream approach)**, **embedded images (JPEG/JP2)**, paragraphs, title/author/page count |
| **HTML** | `html` `htm` `xhtml` | `scraper` | prefers `<article>/<main>/<body>`; headings, paragraphs, lists (nested), tables, code, quotes, images, links, emphasis |
| **Markdown** | `md` `markdown` `mdown` `mkd` | `pulldown-cmark` | **re-normalizes** headings, paragraphs, lists, tables, code, quotes, links, images, emphasis |

Common behavior:
- A leading **UTF-8 BOM is stripped automatically**.
- **Merged cells (rowspan/colspan)** are not supported in the v0.1 scope (the first cell value is preserved).
- **Embedded images** are extracted as `Block::Image` and rendered in Markdown as `![alt](data:...)`. OOXML (docx/pptx) and HWPX embed the original bytes as base64; PDF embeds the JPEG/JP2 stream as-is.
- **PDF table reconstruction** uses a glyph-coordinate alignment heuristic (stream approach), so it works **only for clear grids**. When column alignment is off, it does not treat the region as a table and falls back to body paragraphs (precision over recall).
- For corrupt input, the PDF parser isolates panics and converts them into a `ParseError`.

---

## 8. Semantic Chunking

Instead of naively splitting Markdown into N-token pieces, this uses the **IR's heading boundaries as first-class split points**, then splits only the overflow beyond `max_tokens` at block boundaries. Every chunk carries its **ancestor heading path** (`heading_path`), which raises the quality of hierarchical retrieval and citation.

```rust
pub struct SemanticChunker {
    pub max_tokens:     usize,   // e.g. 512
    pub overlap_tokens: usize,   // e.g. 64 (overlap between adjacent chunks improves recall)
    pub heading_levels: Vec<u8>, // which levels to split on (e.g. [1, 2])
}
impl Default for SemanticChunker { /* 512 / 64 / [1,2] */ }

pub struct Chunk {
    pub heading_path: Vec<String>,   // ["Chapter 1", "Section 1.2"]
    pub content:      String,        // Markdown
    pub token_count:  usize,
    pub metadata:     DocumentMetadata,
}

chunker.chunk(&doc)                          // default token counter
chunker.chunk_with(&doc, &my_token_counter)  // inject a custom counter
```

### Token counting

Token counting is abstracted behind the `TokenCounter` trait. The default is a **dependency-free, multilingual approximation**, [`HeuristicTokenCounter`]:

- Latin / ASCII: roughly 4 characters per token
- CJK (Korean/Chinese/Japanese): roughly 1 token per character

```rust
pub trait TokenCounter { fn count(&self, text: &str) -> usize; }
```

If you need exact counts, implement a `TokenCounter` that wraps `tiktoken-rs` or HuggingFace `tokenizers` and inject it via `chunk_with` (no core change required).

---

## 9. CLI Tool (`rmt`)

Built with `--features cli`.

```bash
cargo build --release --features cli
```

```bash
# Convert a single file (stdout if output is omitted)
rmt convert ./report.docx -o ./report.md

# Batch-convert a directory (recursive, preserves subfolder structure, parallel)
rmt batch ./docs/ -o ./out/ --parallel 8

# Parse to IR -> semantic chunking -> JSONL
rmt chunk ./report.pdf --max-tokens 512 --overlap 64 --heading-levels 1,2 -o ./report.jsonl

# stdin/stdout pipe (format hint required)
cat input.pdf | rmt convert --from pdf > output.md
```

| Subcommand | Arguments | Behavior |
|---|---|---|
| `convert` | `[input]` `-o/--output` `--from <ext>` | single file / stdin to Markdown; stdout if output is omitted |
| `batch` | `<input_dir>` `-o/--output <dir>` `--parallel <N>` | recurse a directory and convert every supported file; `N=0` is auto; parallelized with `rayon` |
| `chunk` | `<input>` `--max-tokens` `--overlap` `--heading-levels` `-o` | emit chunking results as JSONL (one line per chunk) |

---

## 10. Python Bindings (PyO3)

Because most RAG / vector-DB backends run on Python, you can drop this library into the ingestion stage in one line and solve the format-diversity problem in one place. It is built with **abi3 (stable ABI)**, so it is forward-compatible across Python versions.

### Installation (Python users)

```bash
# After PyPI publication: no Rust toolchain needed, just grab the wheel
pip install rust_markdown_transformer

# Before PyPI publication (or to use the latest main): install from GitHub source.
# This path requires a Rust toolchain on the install machine (it compiles the source).
pip install "git+https://github.com/arabangoo/rust_markdown_transformer"
```

Once installed, you can normalize any format to Markdown in one line.

```python
import rust_markdown_transformer as rmt

md     = rmt.convert_to_markdown("report.hwpx")           # Markdown string (for chunking / embedding)
ir     = rmt.convert_to_ir_json("report.pdf")             # IR JSON string (multimodal / citation safety net)
chunks = rmt.convert_to_chunks("report.docx", 512, 64)    # chunk list JSON (for vector-DB ingestion)
ok     = rmt.is_supported("a.xlsx")                       # supported? (True/False)
```

### Building (developers and publishers)

The root `pyproject.toml` (maturin backend) provides the build metadata. Being an abi3 wheel, a single wheel is compatible with Python 3.9+. Thanks to `[tool.maturin] features = ["python"]`, you can omit `--features python`.

```bash
# (a) Development: install into the current venv immediately
pip install maturin
maturin develop --release

# (b) Build a distributable wheel
maturin build --release             # target/wheels/rust_markdown_transformer-*.whl
pip install target/wheels/rust_markdown_transformer-*.whl

# (c) Install straight from GitHub source (requires a Rust toolchain on the install machine)
pip install "git+https://github.com/arabangoo/rust_markdown_transformer"
```

### API

```python
import rust_markdown_transformer as rmt

rmt.__version__                               # "0.1.0"
rmt.supported_parsers()                       # ['docx', 'pptx', 'xlsx', 'hwpx', 'pdf', 'html', 'markdown']
rmt.is_supported("a.docx")                    # True

md   = rmt.convert_to_markdown("report.hwpx") # Markdown string for chunking / embedding
ir   = rmt.convert_to_ir_json("report.hwpx")  # IR JSON string (multimodal / citation safety net)
js   = rmt.convert_to_chunks("report.pdf",    # JSON string of the chunk list
                             max_tokens=512, overlap=64, heading_levels=[1, 2])
```

### Pipeline integration example

```python
# LangChain
from langchain.text_splitter import MarkdownHeaderTextSplitter
import rust_markdown_transformer as rmt
md = rmt.convert_to_markdown("./contract.docx")          # format-agnostic conversion
docs = MarkdownHeaderTextSplitter(headers_to_split_on=[("#","h1"),("##","h2")]).split_text(md)

# Your own pipeline: convert and chunk in one call
import json
for path in Path("./corpus").rglob("*"):
    if rmt.is_supported(str(path)):
        chunks = json.loads(rmt.convert_to_chunks(str(path), 512, 64))
        qdrant.upsert(collection="kb", points=embed(chunks))
```

---

## 11. Embedding into a Service Pipeline (Integration Recipes)

This library is not a standalone app; it is a **core dependency you embed in your ingestion pipeline**. Its core value is absorbing "per-format loader branching" into a single point at the input stage. Pick one of the surfaces below depending on your host environment.

| Host | Surface | Install |
|---|---|---|
| Python RAG (LangChain/LlamaIndex/custom) | Python module | `pip install "git+https://github.com/arabangoo/rust_markdown_transformer"` |
| Rust service | crate | git dependency in `Cargo.toml` |
| Other languages / shell / batch / orchestration | CLI (`rmt`) | `cargo install --git https://github.com/arabangoo/rust_markdown_transformer rust_markdown_transformer --features cli` |

### 11.1 Python RAG pipeline: remove format branching

Replace code that used to branch on format with different loaders by **a single line at the input stage**.

```python
# Before: a separate loader per format (python-docx / pdfminer / pyhwp / BeautifulSoup ...)
# After: one format-agnostic entry point
import rust_markdown_transformer as rmt
md = rmt.convert_to_markdown(path)   # docx/pptx/xlsx/hwpx/pdf/html/md, all of them
```

**LangChain**: wired directly to `MarkdownHeaderTextSplitter`.

```python
from langchain.text_splitter import MarkdownHeaderTextSplitter
import rust_markdown_transformer as rmt

md = rmt.convert_to_markdown("./contract.hwpx")
splitter = MarkdownHeaderTextSplitter(headers_to_split_on=[("#", "h1"), ("##", "h2")])
docs = splitter.split_text(md)        # embedding / indexing continues as usual
```

**LlamaIndex**: wired directly to `MarkdownNodeParser`.

```python
from llama_index.core import Document
from llama_index.core.node_parser import MarkdownNodeParser
import rust_markdown_transformer as rmt

md = rmt.convert_to_markdown("./report.pdf")
nodes = MarkdownNodeParser().get_nodes_from_documents([Document(text=md)])
```

**Custom ingest worker**: walk a corpus, chunk, embed, and load into a vector DB. Unsupported files are skipped, and a file hash makes re-ingestion idempotent.

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
        except RuntimeError as e:           # skip corrupt files and keep going
            print(f"skip {path}: {e}")
            continue
        points = []
        for i, c in enumerate(chunks):
            doc_id = hashlib.sha1(f"{path}:{i}".encode()).hexdigest()  # idempotent upsert on re-run
            points.append({
                "id": doc_id,
                "vector": embed(c["content"]),
                "payload": {
                    "text": c["content"],
                    "heading_path": c["heading_path"],   # hierarchical retrieval / citation metadata
                    "source": str(path),
                    "source_format": c["metadata"]["source_format"],
                },
            })
        collection.upsert(points=points)
```

**Dual-track storage**: load Markdown for retrieval plus IR JSON as a safety net at the same time (for multimodal use and precise citation).

```python
md = rmt.convert_to_markdown(path)        # for embedding / retrieval
ir = rmt.convert_to_ir_json(path)         # preserves merged-cell tables / original structure; keep in object storage
vector_db.upsert(chunks=split(md), metadata={"ir_ref": store_blob(ir)})
```

### 11.2 Embed into a Rust service

```toml
[dependencies]
rust_markdown_transformer = { git = "https://github.com/arabangoo/rust_markdown_transformer", tag = "v0.1.0" }
```

Parsing is synchronous and CPU-bound, so wrap it in `spawn_blocking` inside an async server (axum/actix). Handle uploaded bytes directly with [`parse_reader`](#62-parserregistry) (no need to write a file):

```rust
use std::io::Cursor;
use rust_markdown_transformer::{MarkdownRenderer, ParserRegistry};

// Example axum handler: uploaded document bytes -> Markdown
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

For many files, parallelize the batch with `rayon` (`par_iter` on the consumer side).

### 11.3 Other languages / shell / orchestration: wrap the CLI

From non-Python, non-Rust stacks (Java/Node/Go, and so on) or from batch jobs, call the `rmt` binary as a subprocess.

```bash
# Container / batch: convert an entire directory
rmt batch /data/raw -o /data/markdown --parallel 8

# Pipeline stage: stream chunking JSONL into the next step
rmt chunk /data/raw/report.pdf --max-tokens 512 -o - | my-embedder --stdin
```

```python
# Example: subprocess call from Airflow / cron (language-agnostic integration)
import subprocess
subprocess.run(["rmt", "batch", "./raw", "-o", "./md", "--parallel", "8"], check=True)
```

> It is a single static binary, so you only need to drop `rmt` into your container image. There is no JVM, Node, or Python runtime dependency.

---

## 12. Adding a New Format Parser

You can plug in a parser from a third-party crate without touching the core at all.

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
    fn can_parse_bytes(&self, _h: &[u8]) -> bool { false }   // extension dispatch only

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

For OOXML-family formats (ZIP + XML), the `parsers::ooxml::OoxmlPackage` helper can unzip the package and pull out just the XML parts you need.

---

## 13. Build, Feature Combinations, and Testing

### Cloning the repo and building it yourself

If you clone this repository, you need a **Rust toolchain (stable, 1.74 or newer recommended)** and one build before you can use it. Rust is a compiled language, so source alone cannot be imported or run. Pick one of the three depending on your use case.

| Use case | Build command | Result |
|---|---|---|
| CLI tool | `cargo build --release --features cli` | the single `target/release/rmt` binary ([section 9](#9-cli-tool-rmt)) |
| Python module | `pip install maturin && maturin develop --release` | `import rust_markdown_transformer` installed into the current venv ([section 10](#10-python-bindings-pyo3)) |
| Rust library | add a `path`/`git` dependency in `Cargo.toml` | links into another Rust project ([section 3](#3-installation-and-cargo-features)) |

> A Python build needs the Rust toolchain, `maturin`, and Python headers. To ship to end users without Rust, build a wheel and publish it (for example to PyPI). The build burden falls only on the publisher; users only run `pip install`.

### Building and testing feature combinations

```bash
# Default: all formats + zero FFI (a single static binary)
cargo build --release

# Minimal configuration
cargo build --release --no-default-features --features docx,html,markdown

# CLI binary
cargo build --release --features cli

# Python cdylib
cargo build --release --features python      # or: maturin develop --features python

# Test / lint
cargo test
cargo clippy --all-targets
cargo run --example convert -- ./some.docx   # single-file conversion example
```

The tests **synthesize OOXML/HWPX zips and PDFs inside the test itself**, with no external file dependency, and deterministically verify each parser, the renderer, the chunker, and registry dispatch (`tests/integration.rs`).

---

## 14. Directory Layout

```text
rust_markdown_transformer/
  Cargo.toml
  README.md              # this document
  LICENSE                # Apache-2.0
  src/
    lib.rs               # crate root, re-exports
    ir.rs                # common IR (Document/Block/Inline/Table/...)
    error.rs             # ParseError / ConvertError
    registry.rs          # FormatParser trait + ParserRegistry
    renderer.rs          # IR -> Markdown
    chunker.rs           # SemanticChunker / TokenCounter
    python.rs            # PyO3 bindings (feature = "python")
    bin/
      rmt.rs             # CLI binary (feature = "cli")
    parsers/
      mod.rs             # feature gates + re-exports
      ooxml.rs           # shared OOXML/OWPML zip unpacker (resolves .rels relationships and images)
      media.rs           # shared embedded-image helper (base64, MIME sniffing)
      pdf_layout.rs      # PDF coordinate-based layout (headings, reading order, table reconstruction)
      docx.rs  pptx.rs  xlsx.rs  hwpx.rs  pdf.rs  html.rs  markdown.rs
  examples/
    convert.rs           # single-file conversion example
  tests/
    integration.rs       # synthetic-fixture integration tests
```

---

## 15. License

Apache-2.0
