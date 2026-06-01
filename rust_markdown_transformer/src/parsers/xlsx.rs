//! XLSX 파서 — README §5-1 (XLSX 분기). `calamine` 크레이트 활용.
//!
//! - 시트별로 `Block::Heading { level: 2, text: sheet_name }`.
//! - 사용 영역(used range) → `Block::Table` (첫 행을 헤더로).
//! - 가장자리 빈 행/열 trim. 빈 시트는 건너뜀.

use std::io::{Cursor, Read};

use calamine::{Data, Reader, Xlsx};

use crate::error::ParseError;
use crate::ir::*;
use crate::registry::FormatParser;

const FMT: &str = "xlsx";

pub struct XlsxParser;

impl FormatParser for XlsxParser {
    fn supported_extensions(&self) -> &[&str] {
        &["xlsx", "xlsm"]
    }

    fn name(&self) -> &'static str {
        "xlsx"
    }

    fn can_parse_bytes(&self, header: &[u8]) -> bool {
        header.starts_with(&[0x50, 0x4B, 0x03, 0x04])
    }

    fn parse(&self, input: &mut dyn Read, filename: &str) -> Result<Document, ParseError> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;
        let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(buf))
            .map_err(|e| ParseError::container(FMT, format!("open: {e}")))?;

        let metadata = DocumentMetadata::new(SourceFormat::Xlsx, filename);
        let mut blocks: Vec<Block> = Vec::new();

        for name in wb.sheet_names() {
            let range = wb
                .worksheet_range(&name)
                .map_err(|e| ParseError::container(FMT, format!("sheet '{name}': {e}")))?;

            let grid = trim_grid(
                range
                    .rows()
                    .map(|row| row.iter().map(cell_to_string).collect::<Vec<_>>())
                    .collect(),
            );
            if grid.is_empty() {
                continue;
            }

            blocks.push(Block::Heading { level: 2, text: name });

            let mut rows = grid;
            let headers = rows.remove(0);
            blocks.push(Block::Table(Table { headers, rows, caption: None }));
        }

        Ok(Document { metadata, blocks })
    }
}

/// 셀 값을 문자열로. (calamine `Data` 의 Display 를 사용하되 Empty 는 빈 문자열)
fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 가장자리(상/하/좌/우)의 완전 빈 행과 열을 제거.
fn trim_grid(mut grid: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let row_empty = |r: &Vec<String>| r.iter().all(|c| c.trim().is_empty());

    // 상/하 빈 행 제거.
    while grid.first().is_some_and(row_empty) {
        grid.remove(0);
    }
    while grid.last().is_some_and(row_empty) {
        grid.pop();
    }
    if grid.is_empty() {
        return grid;
    }

    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    // 모든 행 길이를 width 로 패딩.
    for r in &mut grid {
        while r.len() < width {
            r.push(String::new());
        }
    }
    // 좌측 빈 열 제거.
    let mut left = 0;
    while left < width && grid.iter().all(|r| r[left].trim().is_empty()) {
        left += 1;
    }
    // 우측 빈 열 제거.
    let mut right = width; // exclusive
    while right > left && grid.iter().all(|r| r[right - 1].trim().is_empty()) {
        right -= 1;
    }
    if left == 0 && right == width {
        return grid;
    }
    grid.into_iter()
        .map(|r| r[left..right].to_vec())
        .collect()
}
