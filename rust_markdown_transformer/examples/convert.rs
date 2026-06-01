//! 단일 파일을 Markdown 으로 변환해 stdout 으로 출력하는 최소 예제 (README §11).
//!
//! 실행: `cargo run --example convert -- <path>`

use rust_markdown_transformer::ParserRegistry;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: convert <path-to-document>")?;
    let registry = ParserRegistry::with_defaults();
    let md = registry.convert_to_markdown(std::path::Path::new(&path))?;
    println!("{md}");
    Ok(())
}
