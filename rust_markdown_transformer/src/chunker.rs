//! 벡터 DB 최적화 Semantic Chunking 엔진 — README §7.
//!
//! Markdown 을 N-token 으로 무지성 분할하지 않고, **IR 의 Heading 경계를 1급 분할점**으로
//! 사용한 뒤 max_tokens 초과분만 단락 경계로 추가 분할한다. 모든 청크에 조상 헤딩 경로
//! (`heading_path`)를 부여해 계층 검색·citation 품질을 끌어올린다.
//!
//! 토큰 카운팅은 [`TokenCounter`] 로 추상화돼 있다. v0.1 기본값은 의존성 0 인
//! [`HeuristicTokenCounter`] (다국어 근사). 실제 `tokenizers`/`tiktoken-rs` 연동은
//! 향후 feature 로 같은 trait 뒤에 끼운다.

use serde::Serialize;

use crate::ir::{Block, Document, DocumentMetadata};
use crate::renderer::MarkdownRenderer;

/// 텍스트의 토큰 수를 추정/계산하는 추상화.
pub trait TokenCounter {
    fn count(&self, text: &str) -> usize;
}

/// 의존성 0 다국어 근사 토큰 카운터.
///
/// - 라틴/ASCII 어절: 대략 4 글자 ≈ 1 토큰
/// - CJK(한중일) 글자: 글자당 ≈ 1 토큰 (서브워드 토크나이저의 실제 분포에 가깝게)
///
/// 정확한 값이 필요하면 실제 토크나이저를 구현한 [`TokenCounter`] 를 대신 주입한다.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicTokenCounter;

impl TokenCounter for HeuristicTokenCounter {
    fn count(&self, text: &str) -> usize {
        let mut tokens = 0usize;
        let mut latin_run = 0usize;
        for ch in text.chars() {
            if is_cjk(ch) {
                tokens += flush_latin(&mut latin_run);
                tokens += 1;
            } else if ch.is_whitespace() {
                tokens += flush_latin(&mut latin_run);
            } else {
                latin_run += 1;
            }
        }
        tokens += flush_latin(&mut latin_run);
        tokens
    }
}

fn flush_latin(run: &mut usize) -> usize {
    if *run == 0 {
        0
    } else {
        let t = (*run).div_ceil(4); // ceil(run/4), 최소 1
        *run = 0;
        t.max(1)
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF |   // Hangul Jamo
        0x3040..=0x30FF |   // Hiragana/Katakana
        0x3130..=0x318F |   // Hangul Compatibility Jamo
        0x3400..=0x4DBF |   // CJK Ext A
        0x4E00..=0x9FFF |   // CJK Unified
        0xAC00..=0xD7AF |   // Hangul Syllables
        0xF900..=0xFAFF     // CJK Compatibility
    )
}

/// 청킹 설정.
#[derive(Debug, Clone)]
pub struct SemanticChunker {
    /// 청크당 최대 토큰 (예: 512).
    pub max_tokens: usize,
    /// 인접 청크 간 겹침 토큰 (recall 향상, 예: 64).
    pub overlap_tokens: usize,
    /// 어떤 헤딩 레벨에서 자를지 (예: `[1, 2]`).
    pub heading_levels: Vec<u8>,
}

impl Default for SemanticChunker {
    fn default() -> Self {
        SemanticChunker { max_tokens: 512, overlap_tokens: 64, heading_levels: vec![1, 2] }
    }
}

/// 벡터 DB 적재 단위. `heading_path` 가 조상 헤딩 컨텍스트를 제공한다.
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    /// 조상 헤딩 경로 — 예: `["1장", "1.2 절"]`.
    pub heading_path: Vec<String>,
    /// 청크 본문 (Markdown).
    pub content: String,
    /// 토큰 수 (사용된 [`TokenCounter`] 기준).
    pub token_count: usize,
    /// 원본 문서 메타데이터 (모든 청크에 복제).
    pub metadata: DocumentMetadata,
}

impl SemanticChunker {
    /// 기본 [`HeuristicTokenCounter`] 로 청킹.
    pub fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        self.chunk_with(doc, &HeuristicTokenCounter)
    }

    /// 임의의 [`TokenCounter`] 를 주입해 청킹.
    pub fn chunk_with(&self, doc: &Document, counter: &dyn TokenCounter) -> Vec<Chunk> {
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut path_stack: Vec<(u8, String)> = Vec::new();
        let mut cur: Vec<Block> = Vec::new();
        let mut cur_tokens = 0usize;

        for block in &doc.blocks {
            // 1. 분할 대상 헤딩이면 현재 청크를 먼저 flush 하고 경로 갱신.
            if let Block::Heading { level, text } = block {
                if self.heading_levels.contains(level) {
                    self.flush(&mut chunks, &mut cur, &mut cur_tokens, &path_stack, &doc.metadata);
                }
                // 경로 스택 갱신: 같거나 더 깊은 레벨을 pop 후 push.
                while let Some((l, _)) = path_stack.last() {
                    if *l >= *level {
                        path_stack.pop();
                    } else {
                        break;
                    }
                }
                path_stack.push((*level, text.clone()));
            }

            // 2. 블록 토큰 추정 후 누적. max_tokens 초과 시 단락 경계로 분할.
            let block_md = MarkdownRenderer::render_blocks(std::slice::from_ref(block));
            let block_tokens = counter.count(&block_md);

            if cur_tokens + block_tokens > self.max_tokens && !cur.is_empty() {
                let carry = self.flush_with_overlap(
                    &mut chunks,
                    &mut cur,
                    &path_stack,
                    &doc.metadata,
                    counter,
                );
                cur = carry.0;
                cur_tokens = carry.1;
            }

            cur.push(block.clone());
            cur_tokens += block_tokens;
        }

        self.flush(&mut chunks, &mut cur, &mut cur_tokens, &path_stack, &doc.metadata);
        chunks
    }

    /// 현재 누적 블록을 청크로 확정 (overlap 없음 — 헤딩/종료 경계용).
    fn flush(
        &self,
        chunks: &mut Vec<Chunk>,
        cur: &mut Vec<Block>,
        cur_tokens: &mut usize,
        path_stack: &[(u8, String)],
        meta: &DocumentMetadata,
    ) {
        if cur.is_empty() {
            return;
        }
        let content = MarkdownRenderer::render_blocks(cur).trim().to_string();
        if !content.is_empty() {
            chunks.push(Chunk {
                heading_path: path_stack.iter().map(|(_, t)| t.clone()).collect(),
                content,
                token_count: *cur_tokens,
                metadata: meta.clone(),
            });
        }
        cur.clear();
        *cur_tokens = 0;
    }

    /// max_tokens 초과로 분할 → 청크 확정 후, overlap_tokens 만큼 trailing 블록을
    /// 다음 청크 시작으로 carry. carry 된 (블록들, 토큰합) 을 돌려준다.
    fn flush_with_overlap(
        &self,
        chunks: &mut Vec<Chunk>,
        cur: &mut Vec<Block>,
        path_stack: &[(u8, String)],
        meta: &DocumentMetadata,
        counter: &dyn TokenCounter,
    ) -> (Vec<Block>, usize) {
        let content = MarkdownRenderer::render_blocks(cur).trim().to_string();
        let total: usize = counter.count(&content);
        if !content.is_empty() {
            chunks.push(Chunk {
                heading_path: path_stack.iter().map(|(_, t)| t.clone()).collect(),
                content,
                token_count: total,
                metadata: meta.clone(),
            });
        }

        // overlap: 뒤에서부터 블록을 모아 overlap_tokens 에 도달할 때까지 carry.
        let mut carry: Vec<Block> = Vec::new();
        let mut carry_tokens = 0usize;
        if self.overlap_tokens > 0 {
            for block in cur.iter().rev() {
                if carry_tokens >= self.overlap_tokens {
                    break;
                }
                let bt = counter.count(&MarkdownRenderer::render_blocks(std::slice::from_ref(block)));
                carry.push(block.clone());
                carry_tokens += bt;
            }
            carry.reverse();
        }
        cur.clear();
        (carry, carry_tokens)
    }
}
