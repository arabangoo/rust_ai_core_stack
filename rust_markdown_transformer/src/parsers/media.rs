//! 바이너리 미디어(이미지) 공통 헬퍼 — 임베디드 이미지 추출용.
//!
//! OOXML(docx/pptx)·OWPML(hwpx)·PDF 파서가 추출한 이미지 바이트를
//! [`ImageData::Base64`](crate::ir::ImageData) 로 만들 때 공유한다.
//! 외부 의존성 없이(zero-dep) base64 인코딩과 확장자 → MIME 매핑만 제공한다.

/// 바이트열을 표준 base64(RFC 4648, padding 포함) 문자열로 인코딩.
pub fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 0x3F) as usize] as char);
        out.push(T[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

/// 파일 경로/이름의 확장자로 이미지 MIME 추정. 미지 확장자는 `application/octet-stream`.
pub fn mime_from_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "emf" => "image/emf",
        "wmf" => "image/wmf",
        _ => "application/octet-stream",
    }
}

/// 경로에서 디렉토리·확장자를 떼어낸 파일 stem (이미지 alt 기본값).
pub fn stem_of(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 §10 테스트 벡터.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn mime_and_stem() {
        assert_eq!(mime_from_path("ppt/media/image1.PNG"), "image/png");
        assert_eq!(mime_from_path("a/b/pic.jpeg"), "image/jpeg");
        assert_eq!(mime_from_path("x.bin"), "application/octet-stream");
        assert_eq!(stem_of("ppt/media/image12.png"), "image12");
        assert_eq!(stem_of("word/media/photo.jpeg"), "photo");
        assert_eq!(stem_of("noext"), "noext");
    }
}
