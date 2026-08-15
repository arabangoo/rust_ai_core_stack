//! `rpit` 명령줄 도구.
//!
//! 라이브러리를 셸에서 그대로 쓴다. 파이프로 붙일 수 있도록 입력이 없으면 표준 입력을 읽고,
//! `--output` 이 없으면 표준 출력으로 낸다.
//!
//! ```bash
//! rpit detect --text "주민등록번호 880101-1234568"
//! rpit mask --file report.txt --policy tokenize --restore-map map.json --output masked.txt
//! rpit unmask --file masked.txt --restore-map map.json
//! rpit explain --text "접수번호 1234567890123"
//! rpit synth --rounds 2 --format json
//! ```

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use rust_pii_transformer::detect::{detect, Config, Report};
use rust_pii_transformer::mask::{mask_findings, unmask, Policy, PolicySet, Redaction, RestoreMap};
use rust_pii_transformer::synth::corpus;

#[derive(Parser)]
#[command(
    name = "rpit",
    version,
    about = "한국어 개인정보 탐지·마스킹. 모델 없이 규칙과 검증식으로만 판정한다."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 개인정보를 탐지해 결과를 낸다.
    Detect {
        #[command(flatten)]
        input: Input,
        /// 출력 형식.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// 결과를 쓸 파일. 없으면 표준 출력.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// 개인정보를 가린다.
    Mask {
        #[command(flatten)]
        input: Input,
        /// 마스킹 정책.
        #[arg(long, value_enum, default_value_t = PolicyArg::Label)]
        policy: PolicyArg,
        /// 부분 노출에서 앞에 남길 글자 수.
        #[arg(long, default_value_t = 3)]
        keep_prefix: usize,
        /// 부분 노출에서 뒤에 남길 글자 수.
        #[arg(long, default_value_t = 4)]
        keep_suffix: usize,
        /// 덮을 문자.
        #[arg(long, default_value_t = '*')]
        fill: char,
        /// 해시 정책의 열쇠.
        #[arg(long)]
        key: Option<String>,
        /// 토큰화 정책의 복원 맵을 쓸 파일. 이 파일 자체가 개인정보다.
        #[arg(long)]
        restore_map: Option<PathBuf>,
        /// 가려진 텍스트를 쓸 파일. 없으면 표준 출력.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// 토큰화로 가린 텍스트를 원문으로 되돌린다.
    Unmask {
        #[command(flatten)]
        input: Input,
        /// 복원 맵 파일.
        #[arg(long)]
        restore_map: PathBuf,
        /// 결과를 쓸 파일. 없으면 표준 출력.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// 왜 안 걸렸는지까지 보여 준다. 형식은 맞았으나 떨어진 후보를 함께 낸다.
    Explain {
        #[command(flatten)]
        input: Input,
    },

    /// 합성 검증 표본을 만든다. 실제 개인정보를 쓰지 않고 시험하기 위한 것이다.
    Synth {
        /// 엔티티별 반복 횟수.
        #[arg(long, default_value_t = 1)]
        rounds: usize,
        /// 난수 씨앗. 같은 씨앗은 같은 표본을 낸다.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// 출력 형식.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

/// 입력 경로. 셋 다 없으면 표준 입력을 읽는다.
#[derive(Args)]
#[group(multiple = false)]
struct Input {
    /// 명령줄에서 바로 준 텍스트.
    #[arg(long)]
    text: Option<String>,
    /// 읽을 파일.
    #[arg(long)]
    file: Option<PathBuf>,
}

impl Input {
    fn read(&self) -> io::Result<String> {
        if let Some(text) = &self.text {
            return Ok(text.clone());
        }
        if let Some(path) = &self.file {
            return fs::read_to_string(path);
        }
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        Ok(buffer)
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// 사람이 읽는 표.
    Text,
    /// JSON.
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum PolicyArg {
    /// 한국어 이름으로 통째 치환. `[주민등록번호]`
    Label,
    /// 영문 대문자 이름으로 통째 치환. `[CREDIT_CARD]`
    Code,
    /// 글자 수만큼 덮는다.
    Fill,
    /// 앞뒤 일부만 남긴다.
    Partial,
    /// 결정적 가명화. `--key` 가 필요하고 `hash` 기능 플래그로 빌드해야 한다.
    Hash,
    /// 가역. 복원 맵을 함께 낸다.
    Tokenize,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("rpit: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Detect { input, format, output } => {
            let text = input.read()?;
            let report = detect(&text, &Config::default())?;
            let rendered = match format {
                Format::Json => serde_json::to_string_pretty(&report)?,
                Format::Text => render_findings(&text, &report),
            };
            emit(&rendered, output.as_deref())?;
        }

        Command::Mask {
            input,
            policy,
            keep_prefix,
            keep_suffix,
            fill,
            key,
            restore_map,
            output,
        } => {
            let text = input.read()?;
            let policies = build_policies(policy, keep_prefix, keep_suffix, fill, key.as_deref())?;
            let report = detect(&text, &Config::default())?;
            let masked = mask_findings(&text, &report.findings, &policies);

            if let Some(map) = &masked.restore {
                match &restore_map {
                    Some(path) => fs::write(path, serde_json::to_string_pretty(map)?)?,
                    None => {
                        return Err(
                            "토큰화 정책은 --restore-map 이 필요하다. 맵 없이는 되돌릴 수 없다".into()
                        )
                    }
                }
            }

            if !masked.skipped.is_empty() {
                eprintln!(
                    "rpit: 겹쳐서 건너뛴 구간 {} 건 (탐지 층이 겹치는 결과를 냈다)",
                    masked.skipped.len()
                );
            }

            emit(&masked.text, output.as_deref())?;
        }

        Command::Unmask { input, restore_map, output } => {
            let text = input.read()?;
            let raw = fs::read_to_string(&restore_map)?;
            let map: RestoreMap = serde_json::from_str(&raw)?;
            let restored = unmask(&text, &map)?;
            emit(&restored, output.as_deref())?;
        }

        Command::Explain { input } => {
            let text = input.read()?;
            let report = detect(&text, &Config::default())?;
            print!("{}", render_findings(&text, &report));
            println!("\n떨어진 후보 {} 건", report.rejections.len());
            for rejection in &report.rejections {
                let slice = rejection.source.slice(&text);
                println!(
                    "  {:<14} {:<28} 사유 {:?}  점수 {:.2}",
                    rejection.entity.label(),
                    slice,
                    rejection.reason,
                    rejection.score
                );
            }
        }

        Command::Synth { rounds, seed, format } => {
            let samples = corpus(seed, rounds);
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&samples)?),
                Format::Text => {
                    for sample in &samples {
                        let answer = match sample.expected {
                            Some(entity) => entity.label(),
                            None => "없음",
                        };
                        println!("[{:<14}] {}", answer, sample.text);
                    }
                    println!("\n표본 {} 건", samples.len());
                }
            }
        }
    }

    Ok(())
}

fn build_policies(
    policy: PolicyArg,
    keep_prefix: usize,
    keep_suffix: usize,
    fill: char,
    key: Option<&str>,
) -> Result<PolicySet, Box<dyn std::error::Error>> {
    let policy = match policy {
        PolicyArg::Label => Policy::Redact(Redaction::Label),
        PolicyArg::Code => Policy::Redact(Redaction::Code),
        PolicyArg::Fill => Policy::Redact(Redaction::Fill(fill)),
        PolicyArg::Partial => Policy::Partial { keep_prefix, keep_suffix, fill },
        PolicyArg::Tokenize => Policy::Tokenize,
        PolicyArg::Hash => {
            #[cfg(feature = "hash")]
            {
                let key = key.ok_or("해시 정책에는 --key 가 필요하다")?;
                Policy::Hash { key: key.as_bytes().to_vec(), len: 12 }
            }
            #[cfg(not(feature = "hash"))]
            {
                let _ = key;
                return Err("해시 정책은 `--features hash` 로 빌드해야 쓸 수 있다".into());
            }
        }
    };
    Ok(PolicySet::new(policy))
}

fn render_findings(text: &str, report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!("탐지 {} 건\n", report.findings.len()));
    for finding in &report.findings {
        out.push_str(&format!(
            "  {:<14} {:<30} {:?}  점수 {:.2}  바이트 {}..{}  규칙 {}\n",
            finding.entity.label(),
            finding.source.slice(text),
            finding.certainty,
            finding.score,
            finding.source.byte.start,
            finding.source.byte.end,
            finding.evidence.rule,
        ));
        if !finding.evidence.normalizations.is_empty() {
            out.push_str(&format!(
                "  {:<14} 정규화 {}\n",
                "",
                finding.evidence.normalizations.join(", ")
            ));
        }
    }
    out
}

fn emit(content: &str, output: Option<&std::path::Path>) -> io::Result<()> {
    match output {
        Some(path) => fs::write(path, content),
        None => {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            lock.write_all(content.as_bytes())?;
            if !content.ends_with('\n') {
                lock.write_all(b"\n")?;
            }
            Ok(())
        }
    }
}
