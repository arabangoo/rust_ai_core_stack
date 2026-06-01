//! OOXML(Office Open XML) 공통 언패커 — README §5-1.
//!
//! DOCX/PPTX(/추후 XLSX·ODT)는 모두 `ZIP + 내부 XML` 구조다. 이 모듈은 zip 컨테이너를
//! 한 번 풀어 엔트리들을 메모리에 적재하고, 각 포맷 파서가 필요한 XML 파트만 꺼내쓰게 한다.
//!
//! v0.1 은 전체 적재 방식(문서가 대체로 작다). 대용량 streaming(README §12)은 추후 과제.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::error::ParseError;

/// 풀어놓은 OOXML 패키지 — 엔트리 경로 → 바이트.
pub struct OoxmlPackage {
    entries: HashMap<String, Vec<u8>>,
}

impl OoxmlPackage {
    /// reader 에서 zip 을 읽어 모든 파일 엔트리를 메모리에 적재한다.
    pub fn from_reader(input: &mut dyn Read, fmt: &'static str) -> Result<Self, ParseError> {
        let mut buf = Vec::new();
        input.read_to_end(&mut buf)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(buf))
            .map_err(|e| ParseError::container(fmt, format!("zip open: {e}")))?;

        let mut entries = HashMap::new();
        for i in 0..zip.len() {
            let mut f = zip
                .by_index(i)
                .map_err(|e| ParseError::container(fmt, format!("zip entry {i}: {e}")))?;
            if !f.is_file() {
                continue;
            }
            let name = f.name().to_string();
            let mut data = Vec::new();
            f.read_to_end(&mut data)
                .map_err(|e| ParseError::container(fmt, format!("read {name}: {e}")))?;
            entries.insert(name, data);
        }
        Ok(OoxmlPackage { entries })
    }

    /// 엔트리 raw 바이트. (이미지 등 바이너리 파트 추출용 — v0.1 미사용이나 공개 API 로 유지)
    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries.get(name).map(Vec::as_slice)
    }

    /// 엔트리를 UTF-8 문자열로 (OOXML 파트는 UTF-8).
    pub fn get_str(&self, name: &str, fmt: &'static str) -> Result<Option<String>, ParseError> {
        match self.entries.get(name) {
            None => Ok(None),
            Some(bytes) => {
                let s = std::str::from_utf8(bytes)
                    .map_err(|e| ParseError::encoding(fmt, format!("{name}: {e}")))?;
                // UTF-8 BOM 제거 (일부 OOXML/OWPML 파트에 존재).
                Ok(Some(s.strip_prefix('\u{feff}').unwrap_or(s).to_string()))
            }
        }
    }

    /// `prefix` 로 시작하고 `suffix` 로 끝나는 엔트리 경로들을 **자연 정렬**해 반환.
    /// (예: `ppt/slides/slide`, `.xml` → slide1, slide2, … slide10 순서 보장)
    #[cfg(any(feature = "pptx", feature = "hwpx"))]
    pub fn names_matching(&self, prefix: &str, suffix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(prefix) && k.ends_with(suffix))
            .cloned()
            .collect();
        names.sort_by(|a, b| natural_cmp(a, b));
        names
    }
}

/// 숫자 구간을 수치로 비교하는 자연 정렬 (slide2 < slide10).
#[cfg(any(feature = "pptx", feature = "hwpx"))]
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_number(&mut ai);
                    let nb = take_number(&mut bi);
                    match na.cmp(&nb) {
                        std::cmp::Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        ord => return ord,
                    }
                }
            }
        }
    }
}

#[cfg(any(feature = "pptx", feature = "hwpx"))]
fn take_number(it: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(c) = it.peek().copied() {
        if c.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add((c as u8 - b'0') as u64);
            it.next();
        } else {
            break;
        }
    }
    n
}
