//! `rmt` — rust_markdown_transformer CLI (README §10).
//!
//! ```text
//! rmt convert ./report.docx -o ./report.md      # 단일 파일
//! rmt batch ./docs/ -o ./out/ --parallel 8       # 디렉토리 일괄(병렬)
//! rmt chunk ./report.pdf --max-tokens 512 --overlap 64 -o ./report.jsonl
//! cat input.pdf | rmt convert --from pdf > out.md # stdin/stdout 파이프
//! ```
//!
//! `cargo build --release --features cli` 로 빌드된다 (clap + rayon, feature = "cli").

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rust_markdown_transformer::{MarkdownRenderer, ParserRegistry, SemanticChunker};

#[derive(Parser)]
#[command(name = "rmt", version, about = "문서 → 벡터 DB 친화적 Markdown 변환기")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 단일 파일을 Markdown 으로 변환 (입력 생략/`-` 시 stdin, 출력 생략 시 stdout).
    Convert {
        /// 입력 파일 경로 (생략 또는 `-` 면 stdin).
        input: Option<PathBuf>,
        /// 출력 파일 (생략 시 stdout).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 확장자 강제/stdin 포맷 힌트 (예: `--from pdf`).
        #[arg(long)]
        from: Option<String>,
    },
    /// 디렉토리 내 지원 파일 전부를 Markdown 으로 일괄 변환 (병렬).
    Batch {
        /// 입력 디렉토리.
        input: PathBuf,
        /// 출력 디렉토리.
        #[arg(short, long)]
        output: PathBuf,
        /// 워커 스레드 수 (0 = 자동).
        #[arg(long, default_value_t = 0)]
        parallel: usize,
    },
    /// 파일을 IR 로 파싱 후 Semantic Chunking → JSONL 출력.
    Chunk {
        /// 입력 파일 경로.
        input: PathBuf,
        /// 청크당 최대 토큰.
        #[arg(long, default_value_t = 512)]
        max_tokens: usize,
        /// 인접 청크 겹침 토큰.
        #[arg(long, default_value_t = 64)]
        overlap: usize,
        /// 분할 헤딩 레벨 (쉼표 구분, 예: `1,2`).
        #[arg(long, value_delimiter = ',', default_value = "1,2")]
        heading_levels: Vec<u8>,
        /// 출력 파일 (생략 시 stdout).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Convert { input, output, from } => cmd_convert(input, output, from),
        Command::Batch { input, output, parallel } => cmd_batch(input, output, parallel),
        Command::Chunk { input, max_tokens, overlap, heading_levels, output } => {
            cmd_chunk(input, max_tokens, overlap, heading_levels, output)
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rmt: error: {e}");
            ExitCode::FAILURE
        }
    }
}

type CliResult = Result<(), Box<dyn std::error::Error>>;

fn cmd_convert(input: Option<PathBuf>, output: Option<PathBuf>, from: Option<String>) -> CliResult {
    let registry = ParserRegistry::with_defaults();

    let is_stdin = input.is_none() || matches!(&input, Some(p) if p.as_os_str() == "-");
    let md = if is_stdin {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        let doc = registry.parse_reader(&mut buf.as_slice(), "stdin", from.as_deref())?;
        MarkdownRenderer::render(&doc)
    } else {
        registry.convert_to_markdown(input.as_ref().unwrap())?
    };

    write_out(output.as_deref(), md.as_bytes())
}

fn cmd_batch(input: PathBuf, output: PathBuf, parallel: usize) -> CliResult {
    use rayon::prelude::*;

    if parallel > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(parallel)
            .build_global()
            .ok();
    }

    let registry = ParserRegistry::with_defaults();
    let mut files = Vec::new();
    collect_files(&input, &registry, &mut files)?;

    if files.is_empty() {
        eprintln!("rmt: no supported files under {}", input.display());
        return Ok(());
    }
    eprintln!("rmt: converting {} file(s)...", files.len());

    let results: Vec<(PathBuf, Result<(), String>)> = files
        .par_iter()
        .map(|path| {
            let r = (|| -> CliResult {
                let md = registry.convert_to_markdown(path)?;
                let rel = path.strip_prefix(&input).unwrap_or(path);
                let mut out_path = output.join(rel);
                out_path.set_extension("md");
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&out_path, md)?;
                Ok(())
            })();
            (path.clone(), r.map_err(|e| e.to_string()))
        })
        .collect();

    let mut ok = 0usize;
    let mut failed = 0usize;
    for (path, r) in &results {
        match r {
            Ok(()) => ok += 1,
            Err(e) => {
                failed += 1;
                eprintln!("  FAILED {}: {e}", path.display());
            }
        }
    }
    eprintln!("rmt: done — {ok} ok, {failed} failed");
    if failed > 0 {
        return Err(format!("{failed} file(s) failed").into());
    }
    Ok(())
}

fn cmd_chunk(
    input: PathBuf,
    max_tokens: usize,
    overlap: usize,
    heading_levels: Vec<u8>,
    output: Option<PathBuf>,
) -> CliResult {
    let registry = ParserRegistry::with_defaults();
    let doc = registry.parse_to_ir(&input)?;
    let chunker = SemanticChunker { max_tokens, overlap_tokens: overlap, heading_levels };
    let chunks = chunker.chunk(&doc);

    let mut out = String::new();
    for chunk in &chunks {
        out.push_str(&serde_json::to_string(chunk)?);
        out.push('\n');
    }
    write_out(output.as_deref(), out.as_bytes())?;
    eprintln!("rmt: {} chunk(s)", chunks.len());
    Ok(())
}

// ── 헬퍼 ─────────────────────────────────────────────────────

fn write_out(output: Option<&Path>, bytes: &[u8]) -> CliResult {
    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(path, bytes)?;
        }
        None => {
            std::io::stdout().write_all(bytes)?;
        }
    }
    Ok(())
}

/// 디렉토리를 재귀 순회하며 지원되는 파일을 수집.
fn collect_files(dir: &Path, registry: &ParserRegistry, out: &mut Vec<PathBuf>) -> CliResult {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, registry, out)?;
        } else if registry.is_supported(&path) {
            out.push(path);
        }
    }
    Ok(())
}
