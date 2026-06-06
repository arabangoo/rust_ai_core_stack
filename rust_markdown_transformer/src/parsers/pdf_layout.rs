//! PDF Layout 레이어 (프로토타입) — README §5-3 v0.4 목표의 1차 구현.
//!
//! `pdf-extract` 의 저수준 [`OutputDev`] 콜백으로 **글자별 (좌표 + 폰트크기 + 디코딩된 Unicode)**
//! 를 수집한 뒤(한글 CID/ToUnicode 는 pdf-extract 가 처리 → 정확), 그 위에 **순수 Rust clean-room
//! layout 레이어**를 얹는다:
//!
//! 1. 줄 군집화 (y 클러스터) → 줄 단위 텍스트 + bbox + 대표 폰트크기
//! 2. 폰트크기 tier 기반 헤딩 복원 (문서 전체 mode = 본문, 0.5pt 반올림)
//! 3. 고전 XY-Cut 읽기순서 (x 투영 valley 로 컬럼 gutter 감지 → 다단 분리)
//! 4. 단락 조립 (세로 공백 경계로 단락 분리)
//!
//! 알고리즘 출처(§1-5): 고전 XY-Cut(Recursive X-Y Cut, 공개 기술) + 폰트크기 통계 휴리스틱.
//! poppler/MuPDF 코드 미열람. DL 레이아웃 검출기는 zero-FFI 정체성상 미사용(정형 문서 대상).

use std::cmp::Ordering;
use std::panic::{catch_unwind, AssertUnwindSafe};

use pdf_extract::{output_doc, MediaBox, OutputDev, OutputError, Transform};

use crate::ir::{Block, Inline, Table};

/// 수집된 글자 1개 (페이지 상단 기준 top-down 좌표).
#[derive(Clone)]
struct CharBox {
    x: f64,    // 좌측 시작 x
    y: f64,    // 상단 기준 y (아래로 증가)
    size: f64, // 페이지 상 실효 폰트 크기
    adv: f64,  // 페이지 상 가로 advance(글자 폭)
    ch: String,
}

/// 글자 군집 1줄.
struct Line {
    text: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    size: f64, // 대표(중앙값) 폰트 크기
}

/// 페이지별 글자를 모으는 OutputDev.
#[derive(Default)]
struct Collector {
    pages: Vec<Vec<CharBox>>,
    cur: Vec<CharBox>,
    page_top: f64,
}

impl OutputDev for Collector {
    fn begin_page(&mut self, _n: u32, mb: &MediaBox, _art: Option<(f64, f64, f64, f64)>) -> Result<(), OutputError> {
        self.cur = Vec::new();
        self.page_top = mb.ury;
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        self.pages.push(std::mem::take(&mut self.cur));
        Ok(())
    }
    fn output_character(&mut self, trm: &Transform, width: f64, _sp: f64, fs: f64, ch: &str) -> Result<(), OutputError> {
        if ch.is_empty() {
            return Ok(());
        }
        // 공백 글자는 단어 경계 신호로 보존(빈 토큰으로 정규화). 그 외엔 gap 으로도 보조 판정.
        let ch = if ch.trim().is_empty() { " " } else { ch };
        // 텍스트 렌더 행렬의 평행이동 = 글자 위치(PDF 좌표, y 위로 증가). 상단기준으로 뒤집는다.
        let x = trm.m31;
        let y = self.page_top - trm.m32;
        // 페이지 상 실효 폰트 크기 = |transform_vector((fs,fs))| (pdf-extract HTMLOutput 과 동일식)
        let sx = fs * (trm.m11 + trm.m21);
        let sy = fs * (trm.m12 + trm.m22);
        let mut size = (sx * sy).abs().sqrt();
        if !size.is_finite() || size <= 0.0 {
            size = fs.abs().max(1.0);
        }
        // 글자 오른쪽 끝 = x + width * 실효폰트크기 (pdf-extract PlainTextOutput 과 동일식).
        let mut adv = (width * size).abs();
        if !adv.is_finite() || adv <= 0.0 {
            adv = size * 0.5; // 폭 정보 없으면 근사
        }
        self.cur.push(CharBox { x, y, size, adv, ch: ch.to_string() });
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

/// PDF 문서 → layout 기반 IR 블록. 텍스트 레이어가 없으면(스캔) 빈 Vec 반환(호출부에서 fallback).
pub fn layout_blocks(doc: &lopdf::Document) -> Vec<Block> {
    let mut col = Collector::default();
    // output_doc 가 손상 PDF 에서 panic 할 수 있어 격리.
    let res = catch_unwind(AssertUnwindSafe(|| output_doc(doc, &mut col)));
    match res {
        Ok(Ok(())) => {}
        _ => return Vec::new(),
    }

    let char_pages = col.pages; // Vec<Vec<CharBox>> (표 복원에 단어 좌표가 필요해 보존)
    let line_pages: Vec<Vec<Line>> = char_pages.iter().map(|c| group_lines(c.clone())).collect();
    let all_sizes: Vec<f64> = line_pages.iter().flatten().map(|l| round_half(l.size)).collect();
    if all_sizes.is_empty() {
        return Vec::new(); // 텍스트 0 → 스캔 PDF 로 보고 fallback
    }
    let body = mode_size(&all_sizes);
    let tiers = heading_tiers(&all_sizes, body);

    let mut out: Vec<Block> = Vec::new();
    for (chars, lines) in char_pages.iter().zip(line_pages.iter()) {
        if lines.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Block::PageBreak);
        }
        // 표가 검출되면 단락+표를 위→아래로 조립, 아니면 기존 XY-Cut 경로로 폴백
        // (표가 없는 페이지는 기존과 100% 동일한 출력).
        if let Some(mut blocks) = table_aware_page(chars, body, &tiers) {
            out.append(&mut blocks);
        } else {
            let order = xy_cut_order(lines);
            assemble(lines, &order, body, &tiers, &mut out);
        }
    }
    out
}

// ── 1. 줄 군집화 ─────────────────────────────────────────────

fn group_lines(mut chars: Vec<CharBox>) -> Vec<Line> {
    if chars.is_empty() {
        return Vec::new();
    }
    // y 오름차순(상단→하단), 같은 y 면 x.
    chars.sort_by(|a, b| total(a.y, b.y).then(total(a.x, b.x)));

    let mut lines: Vec<Line> = Vec::new();
    let mut bucket: Vec<CharBox> = Vec::new();
    let mut y_ref = chars[0].y;
    let mut size_ref = chars[0].size;

    for c in chars {
        let tol = 0.6 * size_ref.max(c.size);
        if (c.y - y_ref).abs() <= tol {
            // 같은 줄: 가중 평균으로 baseline 보정
            y_ref = (y_ref * bucket.len() as f64 + c.y) / (bucket.len() as f64 + 1.0);
            size_ref = size_ref.max(c.size);
            bucket.push(c);
        } else {
            if let Some(l) = build_line(std::mem::take(&mut bucket)) {
                lines.push(l);
            }
            y_ref = c.y;
            size_ref = c.size;
            bucket.push(c);
        }
    }
    if let Some(l) = build_line(bucket) {
        lines.push(l);
    }
    lines
}

fn build_line(mut chars: Vec<CharBox>) -> Option<Line> {
    if chars.is_empty() {
        return None;
    }
    chars.sort_by(|a, b| total(a.x, b.x));
    let sizes: Vec<f64> = chars.iter().map(|c| c.size).collect();
    let size = median(&sizes);

    let mut text = String::new();
    let mut prev_end: Option<f64> = None; // 직전 글자의 오른쪽 끝(x + advance)
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for c in &chars {
        if let Some(pe) = prev_end {
            // 직전 글자 끝보다 0.1*size 이상 떨어져 시작하면 단어 경계 (pdf-extract 동일 임계).
            if c.x > pe + 0.1 * size && !text.ends_with(' ') {
                text.push(' ');
            }
        }
        text.push_str(&c.ch);
        prev_end = Some(c.x + c.adv);
        x0 = x0.min(c.x);
        x1 = x1.max(c.x + c.adv);
        y0 = y0.min(c.y);
        y1 = y1.max(c.y + c.size);
    }
    let text = collapse_ws(&text);
    if text.is_empty() {
        return None;
    }
    Some(Line { text, x0, y0, x1, y1, size })
}

// ── 2. 폰트크기 tier 헤딩 ───────────────────────────────────

fn round_half(x: f64) -> f64 {
    (x * 2.0).round() / 2.0
}

/// 가장 빈번한 (반올림) 크기 = 본문 크기.
fn mode_size(sizes: &[f64]) -> f64 {
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for &s in sizes {
        if let Some(e) = counts.iter_mut().find(|(v, _)| (*v - s).abs() < 0.01) {
            e.1 += 1;
        } else {
            counts.push((s, 1));
        }
    }
    // 최빈값(동률이면 작은 크기 = 본문일 확률 높음)
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(total(b.0, a.0)))
        .map(|(v, _)| v)
        .unwrap_or(10.0)
}

/// 본문보다 큰 구별되는 크기들을 내림차순으로 = 헤딩 tier.
fn heading_tiers(sizes: &[f64], body: f64) -> Vec<f64> {
    let thresh = body * 1.12;
    let mut tiers: Vec<f64> = Vec::new();
    for &s in sizes {
        if s > thresh && !tiers.iter().any(|t| (*t - s).abs() < 0.01) {
            tiers.push(s);
        }
    }
    tiers.sort_by(|a, b| total(*b, *a)); // 내림차순
    tiers
}

/// 줄 크기 → 헤딩 레벨(없으면 None = 본문).
fn heading_level(size: f64, body: f64, tiers: &[f64]) -> Option<u8> {
    let r = round_half(size);
    if r <= body * 1.12 || tiers.is_empty() {
        return None;
    }
    // 자기보다 큰 tier 개수 = 0-기반 순위.
    let rank = tiers.iter().filter(|&&t| t > r + 0.25).count();
    Some(((rank + 1).min(6)) as u8)
}

// ── 3. 고전 XY-Cut 읽기순서 ─────────────────────────────────

fn xy_cut_order(lines: &[Line]) -> Vec<usize> {
    let med = {
        let mut s: Vec<f64> = lines.iter().map(|l| l.size).collect();
        s.sort_by(|a, b| total(*a, *b));
        median(&s)
    };
    let idx: Vec<usize> = (0..lines.len()).collect();
    let mut order = Vec::with_capacity(lines.len());
    cut(&idx, lines, med, &mut order);
    order
}

/// x 투영 valley(컬럼 gutter)가 있으면 좌→우로 분할 후 각 컬럼 재귀,
/// 없으면 단일 컬럼으로 보고 상→하(같은 y면 좌→우) 정렬.
fn cut(idx: &[usize], lines: &[Line], med: f64, order: &mut Vec<usize>) {
    if idx.len() <= 1 {
        order.extend_from_slice(idx);
        return;
    }
    if let Some((left, right)) = split_columns(idx, lines, med * 1.5) {
        cut(&left, lines, med, order);
        cut(&right, lines, med, order);
        return;
    }
    let mut s = idx.to_vec();
    s.sort_by(|&a, &b| total(lines[a].y0, lines[b].y0).then(total(lines[a].x0, lines[b].x0)));
    order.extend(s);
}

/// x축 구간을 병합해 가장 큰 gutter(>= min_gap)에서 좌/우로 분리. 없으면 None.
fn split_columns(idx: &[usize], lines: &[Line], min_gap: f64) -> Option<(Vec<usize>, Vec<usize>)> {
    // [x0,x1] 구간 수집 후 정렬.
    let mut intervals: Vec<(f64, f64)> = idx.iter().map(|&i| (lines[i].x0, lines[i].x1)).collect();
    intervals.sort_by(|a, b| total(a.0, b.0));
    // 병합하며 최대 gap 추적.
    let mut cur_end = intervals[0].1;
    let mut best_gap = 0.0;
    let mut best_split = f64::NAN; // gap 중앙 x
    for &(s, e) in &intervals[1..] {
        let gap = s - cur_end;
        if gap > best_gap {
            best_gap = gap;
            best_split = cur_end + gap / 2.0;
        }
        if e > cur_end {
            cur_end = e;
        }
    }
    if best_gap < min_gap || !best_split.is_finite() {
        return None;
    }
    let mut left = Vec::new();
    let mut right = Vec::new();
    for &i in idx {
        let cx = (lines[i].x0 + lines[i].x1) / 2.0;
        if cx < best_split {
            left.push(i);
        } else {
            right.push(i);
        }
    }
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

// ── 4. 단락 조립 ─────────────────────────────────────────────

fn assemble(lines: &[Line], order: &[usize], body: f64, tiers: &[f64], out: &mut Vec<Block>) {
    let mut para: Vec<String> = Vec::new();
    let mut last: Option<usize> = None;

    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            let text = collapse_ws(&para.join(" "));
            if !text.is_empty() {
                out.push(Block::Paragraph(vec![Inline::Text(text)]));
            }
            para.clear();
        }
    };

    for &i in order {
        let line = &lines[i];
        if let Some(level) = heading_level(line.size, body, tiers) {
            flush(&mut para, out);
            out.push(Block::Heading { level, text: line.text.clone() });
            last = None;
            continue;
        }
        // 본문 줄: 직전 본문 줄과의 세로 공백/크기 변화로 단락 경계 판단.
        if let Some(p) = last {
            let prev = &lines[p];
            let gap = line.y0 - prev.y1;
            let size_change = (line.size - prev.size).abs() > prev.size * 0.25;
            if gap > 0.7 * line.size || gap < -0.5 * line.size || size_change {
                flush(&mut para, out);
            }
        }
        para.push(line.text.clone());
        last = Some(i);
    }
    flush(&mut para, out);
}

// ── 5. 표 복원 (Stream 방식: 글자 좌표 군집) ──────────────────
//
// 괘선이 없는 표를 행=y 군집·열=x 정렬로 복원한다 (Camelot "stream"/pdfplumber 계열의
// 공개 휴리스틱 기반 clean-room). **명확한 격자만** 표로 인정하고, 한 셀이라도 열에 깔끔히
// 정렬되지 않으면 표로 보지 않는다(정밀도 우선 — 오탐을 줄이고 본문 단락으로 폴백).
// 병합셀·여러 줄 셀은 미지원(README §12). 다단 본문 레이아웃은 표 미검출 시 기존 XY-Cut 으로 처리.

/// 한 행의 셀(큰 가로 공백으로 분리된 텍스트 덩어리). `x0` 는 열 정렬 기준.
struct Cell {
    x0: f64,
    text: String,
}

/// 셀을 보존한 행. `cells.len() >= 2` 면 표 후보 행.
struct Row {
    y0: f64,
    y1: f64,
    size: f64,
    cells: Vec<Cell>,
}

impl Row {
    fn text(&self) -> String {
        collapse_ws(&self.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "))
    }
}

/// 페이지 글자 → 행(셀 보존) 목록. y 군집 기준은 [`group_lines`] 와 동일.
fn group_rows(chars: &[CharBox]) -> Vec<Row> {
    if chars.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<CharBox> = chars.to_vec();
    chars.sort_by(|a, b| total(a.y, b.y).then(total(a.x, b.x)));

    let mut rows: Vec<Row> = Vec::new();
    let mut bucket: Vec<CharBox> = Vec::new();
    let mut y_ref = chars[0].y;
    let mut size_ref = chars[0].size;

    for c in chars {
        let tol = 0.6 * size_ref.max(c.size);
        if (c.y - y_ref).abs() <= tol {
            y_ref = (y_ref * bucket.len() as f64 + c.y) / (bucket.len() as f64 + 1.0);
            size_ref = size_ref.max(c.size);
            bucket.push(c);
        } else {
            if let Some(r) = build_row(std::mem::take(&mut bucket)) {
                rows.push(r);
            }
            y_ref = c.y;
            size_ref = c.size;
            bucket.push(c);
        }
    }
    if let Some(r) = build_row(bucket) {
        rows.push(r);
    }
    rows
}

/// 한 줄의 글자들 → 셀 분할. 가로 공백이 `1.2*size` 보다 크면 셀(열) 경계.
fn build_row(mut chars: Vec<CharBox>) -> Option<Row> {
    if chars.is_empty() {
        return None;
    }
    chars.sort_by(|a, b| total(a.x, b.x));
    let sizes: Vec<f64> = chars.iter().map(|c| c.size).collect();
    let size = median(&sizes);
    let cell_gap = 1.2 * size; // 이보다 큰 가로 공백 = 셀(열) 경계
    let word_gap = 0.1 * size; // 단어 사이 공백

    let mut cells: Vec<Cell> = Vec::new();
    let mut cur = String::new();
    let mut cx0 = f64::NAN;
    let mut prev_end: Option<f64> = None;
    let (mut y0, mut y1) = (f64::MAX, f64::MIN);

    for c in &chars {
        if let Some(pe) = prev_end {
            let gap = c.x - pe;
            if gap > cell_gap {
                push_cell(&mut cells, &mut cur, &mut cx0);
            } else if gap > word_gap && !cur.ends_with(' ') {
                cur.push(' ');
            }
        }
        if cx0.is_nan() {
            cx0 = c.x;
        }
        cur.push_str(&c.ch);
        prev_end = Some(c.x + c.adv);
        y0 = y0.min(c.y);
        y1 = y1.max(c.y + c.size);
    }
    push_cell(&mut cells, &mut cur, &mut cx0);
    if cells.is_empty() {
        return None;
    }
    Some(Row { y0, y1, size, cells })
}

fn push_cell(cells: &mut Vec<Cell>, cur: &mut String, cx0: &mut f64) {
    let text = collapse_ws(cur);
    if !text.is_empty() {
        cells.push(Cell { x0: *cx0, text });
    }
    cur.clear();
    *cx0 = f64::NAN;
}

/// 인접한 표 후보 행(셀>=2)들을 묶어 `(start, end, 열 중심들)` 영역으로.
fn find_table_regions(rows: &[Row]) -> Vec<(usize, usize, Vec<f64>)> {
    let mut regions = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].cells.len() >= 2 {
            let start = i;
            let mut j = i + 1;
            while j < rows.len()
                && rows[j].cells.len() >= 2
                && (rows[j].y0 - rows[j - 1].y1) < 1.5 * rows[j].size
            {
                j += 1;
            }
            if j - start >= 2 {
                if let Some(cols) = infer_columns(&rows[start..j]) {
                    regions.push((start, j, cols));
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    regions
}

/// 영역 행들의 셀 x0 를 열로 군집. 모든 셀이 한 열에 tol 내로 깔끔히 매핑되고
/// (열 중복 없음) 열이 2개 이상이면 `Some(열 중심들)`. 어긋나면 None(표 아님).
fn infer_columns(rows: &[Row]) -> Option<Vec<f64>> {
    let sizes: Vec<f64> = rows.iter().map(|r| r.size).collect();
    let tol = 1.5 * median(&sizes);

    let mut xs: Vec<f64> = rows.iter().flat_map(|r| r.cells.iter().map(|c| c.x0)).collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| total(*a, *b));

    // 1D 그리디 군집: 인접 x0 차이가 tol 이하면 같은 열.
    let mut cols: Vec<f64> = Vec::new();
    let mut group: Vec<f64> = vec![xs[0]];
    for &x in &xs[1..] {
        if x - group[group.len() - 1] <= tol {
            group.push(x);
        } else {
            cols.push(mean(&group));
            group = vec![x];
        }
    }
    cols.push(mean(&group));

    if cols.len() < 2 {
        return None;
    }

    // 검증: 각 행의 모든 셀이 정확히 한 열에 tol 내로 매핑되고 한 행에 같은 열 중복 없음.
    for r in rows {
        let mut used = vec![false; cols.len()];
        for c in &r.cells {
            let k = nearest_col(&cols, c.x0)?;
            if (cols[k] - c.x0).abs() > tol || used[k] {
                return None;
            }
            used[k] = true;
        }
    }
    Some(cols)
}

/// 영역 행들 + 열 중심 → [`Table`]. 헤더는 첫 행, 나머지는 데이터 행.
fn build_table(rows: &[Row], cols: &[f64]) -> Option<Table> {
    let sizes: Vec<f64> = rows.iter().map(|r| r.size).collect();
    let tol = 1.5 * median(&sizes);

    let mut grid: Vec<Vec<String>> = Vec::new();
    for r in rows {
        let mut cells = vec![String::new(); cols.len()];
        for c in &r.cells {
            let k = nearest_col(cols, c.x0)?;
            if (cols[k] - c.x0).abs() > tol {
                return None;
            }
            if cells[k].is_empty() {
                cells[k] = c.text.clone();
            } else {
                cells[k].push(' ');
                cells[k].push_str(&c.text);
            }
        }
        grid.push(cells);
    }
    if grid.len() < 2 || cols.len() < 2 {
        return None;
    }
    let headers = grid.remove(0);
    Some(Table { headers, rows: grid, caption: None })
}

/// 페이지를 행(셀 보존)으로 재구성해 표 영역을 찾는다. 표가 하나라도 있으면 단락+표를
/// 위→아래 순서로 조립해 `Some` 반환. 표가 없으면 `None`(호출부에서 기존 경로로 폴백).
fn table_aware_page(chars: &[CharBox], body: f64, tiers: &[f64]) -> Option<Vec<Block>> {
    let rows = group_rows(chars);
    if rows.len() < 2 {
        return None;
    }
    let regions = find_table_regions(&rows);
    if regions.is_empty() {
        return None;
    }

    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            let t = collapse_ws(&para.join(" "));
            if !t.is_empty() {
                out.push(Block::Paragraph(vec![Inline::Text(t)]));
            }
            para.clear();
        }
    };

    let mut i = 0;
    let mut ri = 0;
    while i < rows.len() {
        if ri < regions.len() && regions[ri].0 == i {
            let (start, end, cols) = &regions[ri];
            flush(&mut para, &mut out);
            if let Some(tbl) = build_table(&rows[*start..*end], cols) {
                out.push(Block::Table(tbl));
            }
            i = *end;
            ri += 1;
            continue;
        }
        let row = &rows[i];
        let text = row.text();
        if !text.is_empty() {
            if let Some(level) = heading_level(row.size, body, tiers) {
                flush(&mut para, &mut out);
                out.push(Block::Heading { level, text });
            } else {
                para.push(text);
            }
        }
        i += 1;
    }
    flush(&mut para, &mut out);

    if out.iter().any(|b| matches!(b, Block::Table(_))) {
        Some(out)
    } else {
        None
    }
}

fn nearest_col(cols: &[f64], x: f64) -> Option<usize> {
    let mut best = 0;
    let mut bd = f64::MAX;
    for (k, &c) in cols.iter().enumerate() {
        let d = (c - x).abs();
        if d < bd {
            bd = d;
            best = k;
        }
    }
    if bd.is_finite() {
        Some(best)
    } else {
        None
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

// ── 유틸 ─────────────────────────────────────────────────────

fn total(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn median(sorted_or_not: &[f64]) -> f64 {
    if sorted_or_not.is_empty() {
        return 0.0;
    }
    let mut v = sorted_or_not.to_vec();
    v.sort_by(|a, b| total(*a, *b));
    v[v.len() / 2]
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
