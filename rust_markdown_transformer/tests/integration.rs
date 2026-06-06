//! 통합 테스트 — 각 파서 + 렌더러 + 청커 + 레지스트리 디스패치.
//!
//! OOXML(docx/pptx/xlsx) 픽스처는 외부 파일 없이 **테스트 안에서 최소 zip 을 직접 합성**해
//! 결정적으로 검증한다. html/markdown 은 문자열 입력으로 충분하다.

use std::io::{Cursor, Write};

use rust_markdown_transformer::{
    Block, Inline, MarkdownRenderer, ParserRegistry, SemanticChunker, SourceFormat,
};

// ── zip 합성 헬퍼 ────────────────────────────────────────────

fn build_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn convert(bytes: Vec<u8>, ext: &str, name: &str) -> String {
    let registry = ParserRegistry::with_defaults();
    let doc = registry
        .parse_reader(&mut Cursor::new(bytes), name, Some(ext))
        .expect("parse ok");
    MarkdownRenderer::render(&doc)
}

// ── Markdown 재정규화 ────────────────────────────────────────

#[test]
fn markdown_renormalize_roundtrip() {
    let src = "# Title\n\nSome **bold** and *italic* and `code`.\n\n- a\n- b\n  - nested\n\n## Section\n\n| H1 | H2 |\n|----|----|\n| a  | b  |\n";
    let md = convert(src.as_bytes().to_vec(), "md", "doc.md");

    assert!(md.contains("# Title"));
    assert!(md.contains("## Section"));
    assert!(md.contains("**bold**"));
    assert!(md.contains("*italic*"));
    assert!(md.contains("`code`"));
    assert!(md.contains("- a"));
    assert!(md.contains("  - nested"), "nested list indented:\n{md}");
    assert!(md.contains("| H1 | H2 |"));
    assert!(md.contains("| --- | --- |"));
    assert!(md.contains("source_format: markdown"));
}

#[test]
fn markdown_strips_utf8_bom() {
    // UTF-8 BOM(EF BB BF) 이 앞에 붙은 입력도 출력에 ﻿ 가 새지 않아야 한다.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("# Title\n\nbody\n".as_bytes());
    let md = convert(bytes, "md", "bom.md");
    assert!(!md.contains('\u{feff}'), "BOM leaked into output:\n{md:?}");
    assert!(md.contains("# Title"));
}

#[test]
fn markdown_idempotent() {
    // IR → md → IR → md 가 안정(결정적)인지.
    let src = "# A\n\ntext one\n\n## B\n\n- x\n- y\n";
    let once = convert(src.as_bytes().to_vec(), "md", "d.md");
    // frontmatter 제외 본문만 추출해 다시 변환.
    let body = once.split("---\n\n").nth(1).unwrap_or(&once).to_string();
    let twice = convert(body.into_bytes(), "md", "d.md");
    let body2 = twice.split("---\n\n").nth(1).unwrap_or(&twice).to_string();
    let body1 = once.split("---\n\n").nth(1).unwrap_or(&once).to_string();
    assert_eq!(body1, body2, "재정규화는 idempotent 해야 한다");
}

// ── HTML ─────────────────────────────────────────────────────

#[test]
fn html_to_markdown() {
    let src = r#"<!doctype html><html><head><title>Doc Title</title>
        <style>.x{}</style></head>
        <body>
          <nav>skip me</nav>
          <article>
            <h1>Heading One</h1>
            <p>Para with <strong>bold</strong> and <a href="https://e.com">link</a>.</p>
            <ul><li>one</li><li>two<ul><li>nested</li></ul></li></ul>
            <pre><code class="language-rust">fn main() {}</code></pre>
            <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
          </article>
        </body></html>"#;
    let md = convert(src.as_bytes().to_vec(), "html", "page.html");

    assert!(md.contains("title: Doc Title"));
    assert!(md.contains("# Heading One"));
    assert!(md.contains("**bold**"));
    assert!(md.contains("[link](https://e.com)"));
    assert!(md.contains("- one"));
    assert!(md.contains("  - nested"), "nested:\n{md}");
    assert!(md.contains("```rust"), "code fence lang:\n{md}");
    assert!(md.contains("fn main() {}"));
    assert!(md.contains("| A | B |"));
    assert!(!md.contains("skip me"), "nav boilerplate removed:\n{md}");
    assert!(!md.contains(".x{}"), "style removed");
}

// ── DOCX (최소 zip 합성) ─────────────────────────────────────

#[test]
fn docx_basic() {
    let styles = r#"<?xml version="1.0"?>
        <w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
          <w:style w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
          <w:style w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>
        </w:styles>"#;
    let document = r#"<?xml version="1.0"?>
        <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
          <w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
            <w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>world</w:t></w:r></w:p>
            <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Sub</w:t></w:r></w:p>
            <w:tbl>
              <w:tr><w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc></w:tr>
              <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>
          </w:body>
        </w:document>"#;
    let core = r#"<?xml version="1.0"?>
        <cp:coreProperties xmlns:cp="x" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:title>My Report</dc:title><dc:creator>Jane</dc:creator>
        </cp:coreProperties>"#;
    let zip = build_zip(&[
        ("word/styles.xml", styles),
        ("word/document.xml", document),
        ("docProps/core.xml", core),
    ]);

    let md = convert(zip, "docx", "r.docx");
    assert!(md.contains("title: My Report"), "core title:\n{md}");
    assert!(md.contains("author: Jane"));
    assert!(md.contains("# Report"));
    assert!(md.contains("## Sub"));
    assert!(md.contains("Hello **world**"), "bold run:\n{md}");
    assert!(md.contains("| H1 | H2 |"));
    assert!(md.contains("| a | b |"));
}

// ── PPTX (최소 zip 합성) ─────────────────────────────────────

#[test]
fn pptx_basic() {
    let slide = |title: &str, body: &str| {
        format!(
            r#"<?xml version="1.0"?>
            <p:sld xmlns:p="p" xmlns:a="a">
              <p:cSld><p:spTree>
                <p:sp>
                  <p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
                  <p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody>
                </p:sp>
                <p:sp>
                  <p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p></p:txBody>
                </p:sp>
              </p:spTree></p:cSld>
            </p:sld>"#
        )
    };
    let s1 = slide("Slide One", "first body");
    let s2 = slide("Slide Two", "second body");
    let zip = build_zip(&[
        ("ppt/slides/slide1.xml", &s1),
        ("ppt/slides/slide2.xml", &s2),
    ]);

    let md = convert(zip, "pptx", "deck.pptx");
    assert!(md.contains("## Slide One"));
    assert!(md.contains("## Slide Two"));
    assert!(md.contains("first body"));
    assert!(md.contains("second body"));
    assert!(md.contains("page_count: 2"));
    // 슬라이드 사이 PageBreak (--- 구분).
    let title_one = md.find("Slide One").unwrap();
    let title_two = md.find("Slide Two").unwrap();
    assert!(md[title_one..title_two].contains("---"), "PageBreak between slides:\n{md}");
}

// ── XLSX (최소 inline-string zip 합성) ──────────────────────

#[test]
fn xlsx_basic() {
    let content_types = r#"<?xml version="1.0"?>
        <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
          <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
          <Default Extension="xml" ContentType="application/xml"/>
          <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
          <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
        </Types>"#;
    let root_rels = r#"<?xml version="1.0"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
        </Relationships>"#;
    let workbook = r#"<?xml version="1.0"?>
        <workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
                  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
          <sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets>
        </workbook>"#;
    let wb_rels = r#"<?xml version="1.0"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
        </Relationships>"#;
    // inline string (t="inlineStr") 로 sharedStrings 회피.
    let sheet1 = r#"<?xml version="1.0"?>
        <worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
          <sheetData>
            <row r="1">
              <c r="A1" t="inlineStr"><is><t>Name</t></is></c>
              <c r="B1" t="inlineStr"><is><t>Age</t></is></c>
            </row>
            <row r="2">
              <c r="A2" t="inlineStr"><is><t>Alice</t></is></c>
              <c r="B2"><v>30</v></c>
            </row>
          </sheetData>
        </worksheet>"#;
    let zip = build_zip(&[
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", root_rels),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", wb_rels),
        ("xl/worksheets/sheet1.xml", sheet1),
    ]);

    let md = convert(zip, "xlsx", "book.xlsx");
    assert!(md.contains("## Data"), "sheet heading:\n{md}");
    assert!(md.contains("| Name | Age |"), "header row:\n{md}");
    assert!(md.contains("| Alice | 30 |"), "data row:\n{md}");
}

// ── HWPX (최소 OWPML zip 합성) ──────────────────────────────

#[test]
fn hwpx_basic() {
    // engName 으로 Outline → 헤딩 레벨 매핑.
    let header = r#"<?xml version="1.0" encoding="UTF-8"?>
        <hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head">
          <hh:styles>
            <hh:style id="0" type="PARA" name="Normal" engName="Normal"/>
            <hh:style id="2" type="PARA" name="Outline1" engName="Outline 1"/>
            <hh:style id="3" type="PARA" name="Outline2" engName="Outline 2"/>
          </hh:styles>
        </hh:head>"#;
    let section0 = r#"<?xml version="1.0" encoding="UTF-8"?>
        <hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section"
                xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
          <hp:p styleIDRef="2"><hp:run><hp:t>보고서 제목</hp:t></hp:run></hp:p>
          <hp:p styleIDRef="0"><hp:run><hp:t>본문 첫 단락입니다.</hp:t></hp:run></hp:p>
          <hp:p styleIDRef="3"><hp:run><hp:t>소제목</hp:t></hp:run></hp:p>
          <hp:tbl>
            <hp:tr>
              <hp:tc><hp:subList><hp:p><hp:run><hp:t>항목</hp:t></hp:run></hp:p></hp:subList></hp:tc>
              <hp:tc><hp:subList><hp:p><hp:run><hp:t>값</hp:t></hp:run></hp:p></hp:subList></hp:tc>
            </hp:tr>
            <hp:tr>
              <hp:tc><hp:subList><hp:p><hp:run><hp:t>매출</hp:t></hp:run></hp:p></hp:subList></hp:tc>
              <hp:tc><hp:subList><hp:p><hp:run><hp:t>100</hp:t></hp:run></hp:p></hp:subList></hp:tc>
            </hp:tr>
          </hp:tbl>
        </hs:sec>"#;
    let content_hpf = r#"<?xml version="1.0" encoding="UTF-8"?>
        <opf:package xmlns:opf="http://www.idpf.org/2007/opf/" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <opf:metadata><dc:title>분기 보고서</dc:title></opf:metadata>
        </opf:package>"#;
    let zip = build_zip(&[
        ("Contents/header.xml", header),
        ("Contents/section0.xml", section0),
        ("Contents/content.hpf", content_hpf),
    ]);

    let md = convert(zip, "hwpx", "report.hwpx");
    assert!(md.contains("title: 분기 보고서"), "content.hpf title:\n{md}");
    assert!(md.contains("# 보고서 제목"), "Outline 1 → h1:\n{md}");
    assert!(md.contains("## 소제목"), "Outline 2 → h2:\n{md}");
    assert!(md.contains("본문 첫 단락입니다."));
    assert!(md.contains("| 항목 | 값 |"), "table header:\n{md}");
    assert!(md.contains("| 매출 | 100 |"), "table row:\n{md}");
}

// ── PDF (lopdf 로 결정적 PDF 합성 → pdf-extract 추출 검증) ──

#[cfg(feature = "pdf")]
#[test]
fn pdf_basic() {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("Hello PDF World")]),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => resources_id,
    });
    let pages = dictionary! {
        "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let info_id = doc.add_object(dictionary! { "Title" => Object::string_literal("My PDF Title") });
    doc.trailer.set("Info", info_id);

    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();

    let md = convert(buf, "pdf", "doc.pdf");
    assert!(md.contains("source_format: pdf"));
    assert!(md.contains("page_count: 1"), "page count from lopdf:\n{md}");
    assert!(md.contains("title: My PDF Title"), "Info title:\n{md}");
    assert!(md.contains("Hello PDF World"), "extracted text:\n{md}");
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_layout_detects_heading_by_font_size() {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    // 큰 폰트(24) 한 줄 + 작은 폰트(10) 본문 한 줄.
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("Big Heading")]),
            Operation::new("Tf", vec!["F1".into(), 10.into()]),
            Operation::new("Td", vec![0.into(), (-40).into()]),
            Operation::new("Tj", vec![Object::string_literal("This is body text content")]),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => resources_id,
    });
    let pages = dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();

    let md = convert(buf, "pdf", "layout.pdf");
    assert!(md.contains("# Big Heading"), "큰 폰트 → 헤딩:\n{md}");
    assert!(md.contains("This is body text content"), "본문 텍스트:\n{md}");
    // 본문은 헤딩(#)으로 잡히면 안 됨.
    assert!(!md.contains("# This is body"), "본문이 헤딩으로 오인되면 안 됨:\n{md}");
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_magic_bytes_detected() {
    use rust_markdown_transformer::FormatParser;
    let p = rust_markdown_transformer::parsers::PdfParser;
    assert!(p.can_parse_bytes(b"%PDF-1.7\n..."));
    assert!(!p.can_parse_bytes(b"PK\x03\x04"));
}

// ── 청커 ─────────────────────────────────────────────────────

#[test]
fn chunker_splits_on_headings() {
    use rust_markdown_transformer::{Document, DocumentMetadata};

    let mut doc = Document::new(DocumentMetadata::new(SourceFormat::Markdown, "x.md"));
    doc.push(Block::Heading { level: 1, text: "Chapter 1".into() });
    doc.push(Block::Paragraph(vec![Inline::text("intro to chapter one")]));
    doc.push(Block::Heading { level: 2, text: "Section 1.1".into() });
    doc.push(Block::Paragraph(vec![Inline::text("section body text")]));
    doc.push(Block::Heading { level: 1, text: "Chapter 2".into() });
    doc.push(Block::Paragraph(vec![Inline::text("second chapter")]));

    let chunker = SemanticChunker { max_tokens: 512, overlap_tokens: 0, heading_levels: vec![1, 2] };
    let chunks = chunker.chunk(&doc);

    assert!(chunks.len() >= 3, "헤딩 경계마다 분할: got {} chunks", chunks.len());
    // heading_path 가 조상 컨텍스트를 담는지.
    let sec = chunks.iter().find(|c| c.content.contains("section body")).unwrap();
    assert_eq!(sec.heading_path, vec!["Chapter 1".to_string(), "Section 1.1".to_string()]);
    let ch2 = chunks.iter().find(|c| c.content.contains("second chapter")).unwrap();
    assert_eq!(ch2.heading_path, vec!["Chapter 2".to_string()]);
}

#[test]
fn chunker_token_count_nonzero() {
    use rust_markdown_transformer::{Document, DocumentMetadata, HeuristicTokenCounter, TokenCounter};
    let c = HeuristicTokenCounter;
    assert!(c.count("hello world foo bar") > 0);
    assert!(c.count("한글 토큰 카운트 테스트") >= 8); // CJK 글자당 ~1 토큰

    let mut doc = Document::new(DocumentMetadata::new(SourceFormat::Markdown, "x.md"));
    doc.push(Block::Paragraph(vec![Inline::text("some content here")]));
    let chunks = SemanticChunker::default().chunk(&doc);
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].token_count > 0);
}

// ── 레지스트리 디스패치 ─────────────────────────────────────

#[test]
fn registry_lists_parsers_and_supports_extensions() {
    let r = ParserRegistry::with_defaults();
    let names = r.parser_names();
    for expected in ["docx", "pptx", "xlsx", "hwpx", "pdf", "html", "markdown"] {
        assert!(names.contains(&expected), "{expected} 파서 등록됨");
    }
}

#[test]
fn registry_file_dispatch_by_extension() {
    // 임시 파일을 통한 실제 경로 디스패치 (convert_to_markdown).
    let dir = std::env::temp_dir();
    let path = dir.join("rmt_test_dispatch.md");
    std::fs::write(&path, "# Hi\n\nbody\n").unwrap();

    let r = ParserRegistry::with_defaults();
    assert!(r.is_supported(&path));
    let md = r.convert_to_markdown(&path).unwrap();
    assert!(md.contains("# Hi"));
    assert!(md.contains("body"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn registry_unsupported_format_errors() {
    let r = ParserRegistry::with_defaults();
    let err = r
        .parse_reader(&mut Cursor::new(b"random".to_vec()), "x.zzz", Some("zzz"))
        .unwrap_err();
    assert!(matches!(
        err,
        rust_markdown_transformer::ConvertError::UnsupportedFormat(_)
    ));
}

// ── 표/이미지 추출 (신규) ─────────────────────────────────────

#[test]
fn pptx_table_extraction() {
    // DrawingML 표(<a:tbl>)가 GFM 표로 추출되는지.
    let cell =
        |t: &str| format!("<a:tc><a:txBody><a:p><a:r><a:t>{t}</a:t></a:r></a:p></a:txBody></a:tc>");
    let slide = format!(
        r#"<?xml version="1.0"?>
        <p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
          <p:graphicFrame><a:graphic><a:graphicData><a:tbl>
            <a:tr>{}{}</a:tr>
            <a:tr>{}{}</a:tr>
          </a:tbl></a:graphicData></a:graphic></p:graphicFrame>
        </p:spTree></p:cSld></p:sld>"#,
        cell("H1"),
        cell("H2"),
        cell("a"),
        cell("b"),
    );
    let zip = build_zip(&[("ppt/slides/slide1.xml", &slide)]);
    let md = convert(zip, "pptx", "deck.pptx");
    assert!(md.contains("| H1 | H2 |"), "pptx table header:\n{md}");
    assert!(md.contains("| --- | --- |"), "pptx table sep:\n{md}");
    assert!(md.contains("| a | b |"), "pptx table row:\n{md}");
}

#[test]
fn pptx_image_extraction() {
    // <p:pic>/<a:blip r:embed> → 슬라이드 .rels 로 media 를 찾아 data URI 이미지로.
    let slide = r#"<?xml version="1.0"?>
        <p:sld xmlns:p="p" xmlns:a="a" xmlns:r="r"><p:cSld><p:spTree>
          <p:pic><p:blipFill><a:blip r:embed="rId1"/></p:blipFill></p:pic>
        </p:spTree></p:cSld></p:sld>"#;
    let rels = r#"<?xml version="1.0"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rId1" Type="img" Target="../media/image1.png"/>
        </Relationships>"#;
    let zip = build_zip(&[
        ("ppt/slides/slide1.xml", slide),
        ("ppt/slides/_rels/slide1.xml.rels", rels),
        ("ppt/media/image1.png", "PNGDATA"),
    ]);
    let md = convert(zip, "pptx", "deck.pptx");
    // alt = media 파일 stem, data URI = image/png base64.
    assert!(
        md.contains("![image1](data:image/png;base64,"),
        "pptx embedded image:\n{md}"
    );
}

#[test]
fn docx_image_extraction() {
    // <w:drawing>/<a:blip r:embed> → document.xml.rels 로 media 를 찾아 data URI 이미지로.
    let document = r#"<?xml version="1.0"?>
        <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                    xmlns:a="a" xmlns:r="r">
          <w:body>
            <w:p><w:r><w:t>before</w:t></w:r></w:p>
            <w:p><w:r><w:drawing><a:blip r:embed="rId5"/></w:drawing></w:r></w:p>
          </w:body>
        </w:document>"#;
    let rels = r#"<?xml version="1.0"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Id="rId5" Type="img" Target="media/photo.jpeg"/>
        </Relationships>"#;
    let zip = build_zip(&[
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", rels),
        ("word/media/photo.jpeg", "JPEGDATA"),
    ]);
    let md = convert(zip, "docx", "r.docx");
    assert!(md.contains("before"), "docx text still parsed:\n{md}");
    assert!(
        md.contains("![photo](data:image/jpeg;base64,"),
        "docx embedded image:\n{md}"
    );
}

#[test]
fn hwpx_image_extraction() {
    // content.hpf 매니페스트 + 본문 binaryItemIDRef → BinData 이미지를 data URI 로.
    let header = r#"<?xml version="1.0"?><hh:head xmlns:hh="hh"></hh:head>"#;
    let content_hpf = r#"<?xml version="1.0"?>
        <opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:manifest>
          <opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/>
        </opf:manifest></opf:package>"#;
    let section = r#"<?xml version="1.0"?>
        <hs:sec xmlns:hs="hs" xmlns:hp="hp" xmlns:hc="hc">
          <hp:p><hp:run><hp:pic><hc:img binaryItemIDRef="image1"/></hp:pic></hp:run></hp:p>
          <hp:p><hp:run><hp:t>hello hwpx</hp:t></hp:run></hp:p>
        </hs:sec>"#;
    let zip = build_zip(&[
        ("Contents/header.xml", header),
        ("Contents/content.hpf", content_hpf),
        ("Contents/section0.xml", section),
        ("BinData/image1.png", "PNGBYTES"),
    ]);
    let md = convert(zip, "hwpx", "doc.hwpx");
    assert!(md.contains("hello hwpx"), "hwpx text still parsed:\n{md}");
    assert!(
        md.contains("![image1](data:image/png;base64,"),
        "hwpx embedded image:\n{md}"
    );
}

#[test]
fn pdf_image_extraction() {
    // DCTDecode(JPEG) XObject 가 data URI 이미지로 추출되는지.
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    // DCTDecode 이미지 XObject (내용은 추출 검증용 더미 바이트 — base64 대상이면 충분).
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        b"FAKEJPEGDATA".to_vec(),
    ));
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
        "XObject" => dictionary! { "Im1" => image_id },
    });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("Picture page")]),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => resources_id,
    });
    let pages = dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();

    let md = convert(buf, "pdf", "pic.pdf");
    assert!(
        md.contains("![image-") && md.contains("data:image/jpeg;base64,"),
        "pdf embedded image:\n{md}"
    );
}

#[test]
fn pdf_table_reconstruction() {
    // 괘선 없는 격자(2열 x 3행)를 좌표로 배치 → Stream 방식 표 복원이 GFM 표를 만드는지.
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });

    // 텍스트를 절대 위치(Tm)로 찍어 2개의 열(x=72, x=320) x 3개의 행(y=700/680/660) 격자 구성.
    let cell = |x: i64, y: i64, s: &str| -> Vec<Operation> {
        vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new(
                "Tm",
                vec![1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()],
            ),
            Operation::new("Tj", vec![Object::string_literal(s)]),
            Operation::new("ET", vec![]),
        ]
    };
    let mut ops = Vec::new();
    for (x, s) in [(72_i64, "Name"), (320, "Age")] {
        ops.extend(cell(x, 700, s));
    }
    for (x, s) in [(72_i64, "Alice"), (320, "30")] {
        ops.extend(cell(x, 680, s));
    }
    for (x, s) in [(72_i64, "Bob"), (320, "25")] {
        ops.extend(cell(x, 660, s));
    }
    let content = Content { operations: ops };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => resources_id,
    });
    let pages = dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();

    let md = convert(buf, "pdf", "table.pdf");
    assert!(md.contains("| Name | Age |"), "pdf table header:\n{md}");
    assert!(md.contains("| Alice | 30 |"), "pdf table row 1:\n{md}");
    assert!(md.contains("| Bob | 25 |"), "pdf table row 2:\n{md}");
}
