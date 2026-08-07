# rust_ai_serving_engine

> **A Rust-based local AI model serving engine**
>
> It moves a GGUF model downloaded from Hugging Face through `register -> verify integrity -> load -> infer -> serve over an OpenAI-compatible API`.
> The local model experience offered by Ollama, llama.cpp, and LM Studio is reimplemented here
> as a **pure-Rust single binary plus a one-line Python import**.

This document is the engine's **complete developer manual**. It covers the design principles, the public API, the supported models,
chat templates and generation control, HTTP/CLI/Python usage, service integration, how to add a new architecture, and the build/test procedures.

[Key reference papers]

1. Attention Is All You Need (the origin of the Transformer architecture): https://arxiv.org/abs/1706.03762
2. LLaMA: Open and Efficient Foundation Language Models (the Llama-family decoder architecture): https://arxiv.org/abs/2302.13971
3. Efficiently Scaling Transformer Inference (the origin of the model that decomposes inference into prefill and decode phases and analyzes performance by compute and memory-bandwidth bounds; the theoretical basis for RASE_PROFILE phase profiling): https://arxiv.org/abs/2211.05102
4. The Case for 4-bit Precision: k-bit Inference Scaling Laws (the basis for 4-bit quantized inference): https://arxiv.org/abs/2212.09720
5. Efficient Memory Management for Large Language Model Serving with PagedAttention (LLM serving and KV cache management): https://arxiv.org/abs/2309.06180

---

## Table of Contents

1. [Core Features](#1-core-features)
2. [Quick Start](#2-quick-start)
3. [Installation and Cargo Features](#3-installation-and-cargo-features)
4. [Architecture](#4-architecture)
5. [Model Manifest and Registry](#5-model-manifest-and-registry)
6. [Public API Reference](#6-public-api-reference)
7. [Supported Models](#7-supported-models)
8. [Chat Templates and Generation Control](#8-chat-templates-and-generation-control)
9. [HTTP API (OpenAI-compatible)](#9-http-api-openai-compatible)
10. [CLI Tools](#10-cli-tools)
11. [Python Binding (PyO3)](#11-python-binding-pyo3)
12. [Embedding into a Service Pipeline](#12-embedding-into-a-service-pipeline)
13. [Adding a New Model Architecture](#13-adding-a-new-model-architecture)
14. [Build, Features, and Tests](#14-build-features-and-tests)
15. [Directory Structure](#15-directory-structure)
16. [License and Model Responsibility](#16-license-and-model-responsibility)

---

## 1. Core Features

The most underrated part of running a local large language model (LLM) is the **model lifecycle and the serving contract**.
No matter how good the inference kernel is, if "which file is an executable model, what is loaded in memory right now,
and whether the same input yields the same output" is not managed, local AI turns into an irreproducible toy.
Instead of writing a new inference kernel, this engine aims to be the **runtime framework** that owns the systems engineering above and below the kernel.

| Principle | Meaning |
|---|---|
| **Assemble the kernel, do not build it** | Tensor operations and model implementations come from Candle, Hugging Face's Rust framework. The engine's differentiator is the model lifecycle (register, verify, load, cache, unload) and the serving contract. The one exception is CPU prefill, which has its own hybrid kernel path (below). |
| **Hybrid prefill plus GQA decode** | Candle's quantized matmul re-dequantizes the entire weight for every prompt token, which makes prefill as slow as decode. For long prompts the engine dequantizes each layer's weights only once and processes them with an f32 matmul (GEMM), while decode keeps the memory-optimal quantized kernel. Attention over a long prompt is also handled by a custom blocked kernel (16 query rows share the K/V reads, computed with an exact online softmax). On a 16-core AVX2 laptop, the first token for a 1,500-character document context drops from 141s to 20s, and 4,000 characters from 97s to 48s. Decode attention likewise uses a custom kernel for GQA models with a high KV sharing ratio (query:KV above 2:1, e.g. Qwen3-4B at 32:8): it reads the KV once and updates every query head in the group, removing redundant reads. |
| **The manifest is the contract** | A model file is only executable through a TOML manifest that records its SHA-256 hash, architecture, tokenizer, and chat template. It separates "an executable model" from "just a big file". |
| **Deterministic generation** | The same model, prompt, seed, and sampling settings produce the same output. A fixed-seed sampler and a deterministic generation loop make regression testing possible. |
| **Load once, reuse continuously** | A process-global session cache performs hash verification and model loading only on the first call. It does not re-read several GB per request. |
| **Pure Rust, zero external runtime** | This is not a wrapper around C++ llama.cpp. It runs as a single binary with no Python, Node.js, or external process, and Python attaches as a PyO3 extension module. |

### What is the same as Ollama, and what is different

The user-experience goal is the same: get a model, register it, and chat locally. The implementation philosophy differs.

- Ollama is a Go server wrapping llama.cpp (C++). This engine is **Rust across every layer**, so it is assembled
  type-safely in a single Cargo workspace, and the library, CLI, and Python extension share the same core.
- Model management is an **explicit manifest** rather than an implicit cache. Weight and tokenizer hashes are recorded,
  integrity is verified before load, and the cache is invalidated automatically when a file changes.
- Embedding is a first-class scenario. Without starting a separate server, you can run inference directly
  **inside the host service process** as a Rust crate or a Python module.

---

## 2. Quick Start

All three surfaces (CLI server, Python, Rust) follow the same flow: get a model, register it, attach a tokenizer, and generate.

### CLI: from pulling a model to an OpenAI-compatible server

```bash
cargo build --release

# 1) Download weights + tokenizer.json from Hugging Face and register as an executable bundle
./target/release/rust-ai-serving-engine model pull \
  --repo unsloth/Qwen3-4B-Instruct-2507-GGUF \
  --file Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  --id qwen3-4b \
  --architecture qwen3 \
  --tokenizer-repo Qwen/Qwen3-4B-Instruct-2507 \
  --tokenizer-file tokenizer.json

# 2) Start the OpenAI-compatible server
./target/release/rust-ai-serving-engine serve --port 8080
```

```bash
# 3) Chat with any OpenAI client (streaming with "stream": true)
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "qwen3-4b", "messages": [{"role": "user", "content": "Hello?"}]}'
```

### Python

```python
import rust_ai_serving_engine as engine

# Register (once): download weights + link a local tokenizer.json
engine.pull_model("./models", "unsloth/Qwen3-4B-Instruct-2507-GGUF",
                  "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", "qwen3-4b", architecture="qwen3")
engine.attach_tokenizer("./models", "qwen3-4b", "./tokenizer.json")

# Chat: chat template and stop token applied automatically, model stays resident in the process cache
answer = engine.generate_chat_registered_gguf(
    "./models", "qwen3-4b",
    [{"role": "user", "content": "Introduce yourself in one sentence."}],
    max_tokens=64,
)
print(answer)
```

### Rust library

```rust
use rust_ai_serving_engine_core::{DevicePreference, GenerationConfig, ModelRegistry, generate};
use rust_ai_serving_engine_models::{ChatMessage, SessionCache};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = ModelRegistry::open("./models")?;
    let cache = SessionCache::new();

    // First call: hash verification + load / afterwards: reuse the memory-resident session
    let session = cache.get_or_load(&registry, "qwen3-4b", DevicePreference::Auto)?;
    let mut session = session.lock().unwrap();

    let template = session.chat_template.expect("chat template resolved from manifest");
    let prompt = template.render(&[ChatMessage {
        role: "user".into(),
        content: "What is the capital of France?".into(),
    }])?;
    let prompt_tokens = session.tokenizer.encode(&prompt, false)?;

    let mut config = GenerationConfig::default();
    if let Some(eos) = session.eos_token {
        config.stop_tokens.push(eos); // generation stops automatically at the stop token
    }
    let result = generate(session.decoder.as_mut(), &prompt_tokens, &config, || false)?;
    println!("{}", session.tokenizer.decode(&result.tokens, true)?);
    Ok(())
}
```

---

## 3. Installation and Cargo Features

In a Rust project's `Cargo.toml`:

```toml
[dependencies]
rust-ai-serving-engine-core = { git = "https://github.com/arabangoo/rust_ai_serving_engine" }
rust-ai-serving-engine-models = { git = "https://github.com/arabangoo/rust_ai_serving_engine" }
```

### Workspace crates

| Crate | Role | Key dependencies |
|---|---|---|
| `rust_ai_serving_engine_core` | Manifest, registry, generation loop, sampler, device selection, error contract | `candle-core`, `hf-hub`, `sha2` |
| `rust_ai_serving_engine_models` | GGUF decoders (Llama, Qwen3), tokenizer, chat templates, session cache | `candle-transformers`, `tokenizers` |
| `rust_ai_serving_engine_api` | OpenAI-compatible HTTP API and SSE streaming | `axum`, `tokio` |
| `rust_ai_serving_engine_cli` | `model`, `runtime`, `serve` command line (binary name `rust-ai-serving-engine`) | `clap` |
| `rust_ai_serving_engine_python` | PyO3 extension module (module name `rust_ai_serving_engine`) | `pyo3` (abi3) |

### Feature list

| Feature | Crate | Enables | Notes |
|---|---|---|---|
| **`cpu`** | core, models | CPU execution (enabled by default) | Pure Rust, no external runtime |
| `cuda` | core, models | NVIDIA GPU execution path | Forwards `candle-core/cuda` |
| `metal` | core, models | Apple Silicon GPU execution path | Forwards `candle-core/metal` |
| **`python`** | python | PyO3 cdylib binding | Enabled automatically by maturin |

> The default (CPU) build requires no external shared library or subprocess. With just the model file and a single binary,
> it runs in offline and air-gapped environments (Hugging Face download is only needed when using `model pull`).

---

## 4. Architecture

```text
request (HTTP/CLI/Python)
  -> registry: look up the manifest (architecture, tokenizer, template, hash)
  -> session cache: hash verification + load the decoder and tokenizer, only once
  -> render the chat template -> tokenize
  -> prefill: evaluate the whole prompt + build the KV cache
  -> decode: sample a token -> update the KV cache, repeat (stop token, stop string, cancellation check)
  -> token callback -> send an SSE delta or assemble the completed text
```

The heart of it is the **separation of contracts**. `core` contains no inference backend; it only defines the manifest, the generation loop, and the traits.
`models` is the Candle implementation of that contract. HTTP, CLI, and Python are just three surfaces over the same `models`.

- **Register**: [`ModelRegistry`](#61-modelregistry-core) hashes the weights and writes the TOML manifest atomically.
- **Load**: [`SessionCache`](#63-modelsession-and-sessioncache-models) reuses the memory-resident session when the manifest hash matches, and verifies and reloads when it differs.
- **Generate**: [`generate` / `generate_with`](#62-generation-contract-core) runs the architecture-neutral decode loop. The decoder owns the KV (Key-Value) cache.
- **Serialize**: concurrent requests for the same model are serialized by the session mutex (the KV cache cannot be shared). Different models generate concurrently.

---

## 5. Model Manifest and Registry

A model store is a single folder. Under `manifests/`, one TOML file is written per model.

```toml
id = "qwen3-4b"
kind = "generator"                 # generator | embedding
format = "gguf"                    # gguf | safetensors
weights = "<absolute path to the weight file>"
sha256 = "<weight SHA-256>"
tokenizer = "<absolute path to tokenizer.json>"
tokenizer_sha256 = "<tokenizer SHA-256>"
architecture = "qwen3"
context_length = 262144
chat_template = "chatml"           # chatml | llama3 | mistral (defaults to the architecture default if omitted)
```

The manifest is the contract that distinguishes an executable model from a plain file:

- **Integrity**: `verify` recomputes the SHA-256 of the weights and tokenizer and compares against the manifest.
  The session cache runs the same check at load time, and if the hash has changed it discards the cache and reloads.
- **Executable conditions**: generation requires three things, `format = "gguf"` + `architecture` + `tokenizer`.
  If any is missing, the load stage states exactly what is absent and refuses.
- **Weight file location**: `model pull` points the manifest at the file it downloaded into the Hugging Face cache.
  Clearing the cache means the model must be downloaded again. For a model you want to keep, move it to a folder of your choice and register it with `model import`.

---

## 6. Public API Reference

### 6.1 `ModelRegistry` (core)

```rust
ModelRegistry::open(root) -> Result<Self>          // open the store folder (create if absent)

fn import_local(&self, id, weights, kind: ModelKind,
                architecture: Option<String>, context_length: Option<u32>,
                chat_template: Option<String>) -> Result<ImportedModel>
fn attach_tokenizer(&self, id, tokenizer_path) -> Result<ModelManifest>
fn get(&self, id) -> Result<ModelManifest>         // look up the manifest (no hash recompute)
fn list(&self) -> Result<Vec<ModelManifest>>       // list sorted by id
fn verify(&self, id) -> Result<ModelManifest>      // re-verify the weight and tokenizer hashes
```

`HuggingFaceHub::download(repo, file) -> Result<PathBuf>` downloads a public file from the Hugging Face Hub
into the managed cache (core, based on `hf-hub`).

### 6.2 Generation Contract (core)

```rust
/// The architecture-neutral decoder contract implemented by a loaded model.
pub trait TokenDecoder: Send {
    fn prefill(&mut self, prompt: &[u32]) -> Result<Vec<f32>>;  // init the KV cache + first logits
    fn decode(&mut self, token: u32) -> Result<Vec<f32>>;       // evaluate one token -> next logits
    fn eos_token(&self) -> Option<u32> { None }                 // the stop token declared by the model file
}

pub struct GenerationConfig {
    pub max_tokens: usize,        // default 256
    pub temperature: f32,         // default 0.7 (0.0 = greedy selection)
    pub top_k: Option<usize>,     // default Some(40)
    pub seed: u64,                // default 0 - the same seed gives the same output
    pub stop_tokens: Vec<u32>,    // generation stops immediately when this token appears
}

// Completion-style generation: aborts when the cancel callback returns true
generate(decoder, prompt, &config, cancelled) -> Result<GenerationResult>

// For streaming: on_token is called per token, and returning false stops decoding
generate_with(decoder, prompt, &config, cancelled, on_token) -> Result<GenerationResult>

pub struct GenerationResult { pub tokens: Vec<u32>, pub stop_reason: GenerationStopReason }
pub enum GenerationStopReason { MaxTokens, StopToken, Cancelled }
```

### 6.3 `ModelSession` and `SessionCache` (models)

```rust
/// One loaded model: decoder + tokenizer + stop token + chat template.
pub struct ModelSession {
    pub decoder: Box<dyn TokenDecoder>,
    pub tokenizer: LocalTokenizer,
    pub eos_token: Option<u32>,
    pub chat_template: Option<ChatTemplate>,
}
ModelSession::load(&manifest, &runtime) -> Result<Self>

/// Process-global session cache. Key = model id + device.
SessionCache::new() -> Self
fn get_or_load(&self, registry, id, device: DevicePreference)
    -> Result<Arc<Mutex<ModelSession>>>   // reuse if the hash is unchanged, verify and reload if changed
fn clear(&self)                            // unload everything (free memory)
```

### 6.4 Decoder and Tokenizer (models)

```rust
// Select a GGUF decoder by the registered architecture name
load_gguf_decoder(architecture, weights, &runtime) -> Result<Box<dyn TokenDecoder>>

LlamaGgufDecoder::load(path, &runtime) -> Result<Self>   // Llama/Mistral-compatible GGUF
Qwen3GgufDecoder::load(path, &runtime) -> Result<Self>   // Qwen3-compatible GGUF

LocalTokenizer::from_file(path) -> Result<Self>           // Hugging Face tokenizer.json
fn encode(&self, text, add_special_tokens: bool) -> Result<Vec<u32>>
fn decode(&self, tokens, skip_special_tokens: bool) -> Result<String>
```

### 6.5 Device Selection (core)

```rust
pub enum DevicePreference { Auto, Cpu, Cuda, Metal }   // Auto = CUDA -> Metal -> CPU fallback

RuntimeDevice::select(preference) -> Result<RuntimeDevice>
fn smoke_test(&self) -> Result<()>     // confirm the backend works with a real tensor op
fn is_accelerated(&self) -> bool
```

### 6.6 Error Types (core)

```rust
pub enum EngineError {
    InvalidModelId(String), UnsupportedFormat(String), UnsupportedArchitecture(String),
    ModelNotFound(String), ModelFileNotFound(String),
    IntegrityMismatch { id, expected, actual },
    BackendUnavailable(String), Candle(String), Tokenizer(String), HuggingFaceHub(String),
    InvalidGenerationConfig(String), InvalidLogits,
    Io(std::io::Error), TomlSerialize(..), TomlDeserialize(..),
}
```

> Load failures are distinguished by cause. A corrupt file, a hash mismatch, an unsupported architecture, and a missing backend
> are each reported as a different error, so the caller can pass "why it failed" straight through to the user.

---

## 7. Supported Models

The execution format is GGUF quantized models, and the architecture name selects the decoder.

| Architecture (`--architecture`) | Decoder | Representative models |
|---|---|---|
| `qwen3` | `Qwen3GgufDecoder` (Candle quantized_qwen3) | Qwen3-1.7B, Qwen3-4B-Instruct-2507. Verified end to end with real models: chat completion, SSE streaming, Korean multi-byte characters, and session-cache reuse. Measured decode (16-core hybrid CPU laptop, short context): 1.7B q4 about 40-46 tokens/s, 4B q4 about 20 tokens/s (with the 8-thread decode policy applied). |
| `llama` `llama2` `llama3` `mistral` `mixtral` | `LlamaGgufDecoder` (Candle quantized_llama) | Llama 2/3, Mistral, Mixtral instruct family |

Operational notes:

- **Unsupported architectures are refused with a clear error instead of wrong output.** `qwen2` returns an error
  guiding you to use a Qwen3 GGUF, and `phi` returns an error stating why it is excluded. Safetensors can be registered and
  hash-verified in the registry, but execution runs on GGUF.
- **Qwen3 hybrid (thinking) models are handled with `/no_think`.** The Qwen3 base editions (0.6B, 1.7B, and so on) are hybrid
  models that emit a `<think>` reasoning block before answering. For CPU serving, the standard approach is to disable reasoning
  by adding Qwen's official soft switch `/no_think` to the system prompt, and this is how the Qwen3-1.7B production run was
  validated. However, **the engine's ChatML template does not filter out `<think>` blocks, so handling any residual tags is the
  caller's responsibility.** Even in the `/no_think` state, variants such as an empty block (`<think></think>`), a `</think>`
  with no opening tag, or a duplicated closing tag are observed at the head of the stream, so a caller-side filter is needed.
  The thinking-removed instruct variants (Qwen3-4B-Instruct-2507 and similar) work with the ChatML template as is, without such
  handling (verified).
- **The same model is serialized during generation.** Because the KV cache cannot be shared across requests, it is processed
  sequentially via the session mutex. Different models generate concurrently. Large multi-user batching is a non-goal of this
  engine (that is vLLM's domain).
- **The tokenizer uses an external `tokenizer.json`.** If a quantized GGUF repository has no tokenizer.json, download it from the
  original model repository and attach it (`model pull --tokenizer-repo` handles this in one step).

---

## 8. Chat Templates and Generation Control

An instruct model works correctly only when the conversation markup used during training is reproduced exactly. The engine renders
a list of conversation messages into the per-model markup and ends generation automatically at the stop token.

### Template selection rules

1. If the manifest's `chat_template` value (`chatml` | `llama3` | `mistral`) is present, use it.
2. Otherwise use the architecture default: `qwen3` -> ChatML, `llama3` -> Llama3, `llama`/`llama2`/`mistral`/`mixtral` -> Mistral `[INST]`.
3. If neither is present, the chat request is refused (the completion API works without a template).

| Template | Markup | Target |
|---|---|---|
| `chatml` | `<|im_start|>role ... <|im_end|>` | Qwen family, many ChatML fine-tunes |
| `llama3` | `<|start_header_id|>role<|end_header_id|> ... <|eot_id|>` | Llama 3 instruct |
| `mistral` | `<s>[INST] ... [/INST]` (system is merged into the following user turn) | Mistral/Llama 2 instruct |

Because the template writes the special tokens directly, the chat prompt is encoded without the tokenizer's automatic special tokens.

### Automatic stop at the end-of-sequence (EOS) token

The `tokenizer.ggml.eos_token_id` from the GGUF metadata is read at load time, and the HTTP and Python chat surfaces
add it to the stop tokens automatically. The user does not need to know the token id.

### stop strings

The OpenAI-compatible `stop` (a single string or an array) is supported. When a stop string appears in the generated text,
only the text up to just before it is returned and generation ends. In streaming, text is held back by the length of the
stop string, so **a stop string that straddles a chunk boundary does not leak to the client either.**

### Sampling

- `temperature = 0.0`: deterministic greedy selection (for regression testing)
- `temperature > 0` + `top_k`: probability sampling based on a fixed seed (`seed`). The same seed gives the same output
- When a multi-byte character (such as Korean) straddles a token boundary, emission is deferred until it is complete, so a broken character never goes out on the stream

### wgpu prefill GEMM offload (experimental, opt-in via RASE_GPU=1)

An experimental path that offloads the quantized linear layer (Q4_K) of prefill to the GPU. It keeps the quantized weights
resident on the GPU per matrix and dequantizes and multiplies inside a WGSL shader (copying the dequantized f32 on every
call would be several GB, which is a loss). Decode is memory-bandwidth bound with no gain on an integrated GPU, so it always
stays on the CPU.

- **How to enable**: `RASE_GPU=1` (disabled by default). The current f32 shader runs about the same speed as the CPU
  hybrid GEMM on an integrated GPU, so the default is off. It will switch to on by default once an f16 shader path
  outperforms the CPU
- **Safeguards**: software adapters (WARP, llvmpipe class, DeviceType Cpu) are excluded automatically / on a runtime
  failure (device loss, mapping failure) the whole path falls back to the CPU immediately / dtypes other than Q4_K
  (Q6_K and so on) and GPU-absent environments fall back to the CPU per matrix
- **Diagnostics**: Python `gpu_info()` returns `active: <adapter>` / `fallback(runtime-failure)` /
  `inactive`. The profiling counters `gemm_gpu_ns` and `gemm_gpu_calls` measure the offloaded share (see the performance
  profiling section above)
- **Numerical characteristics**: the GPU dequant GEMM is not bit-identical to the CPU because of the operation order, but
  it matches within the logit tolerance (identical greedy-decode output under the same seed was confirmed by measurement)

### Decode thread policy (CANDLE_NUM_THREADS)

The quantized matvec and fused attention of decode (token generation) run over a barrier pool sized by `CANDLE_NUM_THREADS`,
with static even partitioning. On a hybrid CPU (a mix of performance cores, efficiency cores, and low-power efficiency
cores), every barrier waits for the slowest core, so the default of using all cores actually halves decode throughput
(measured on a 16-core Core Ultra 7 255H: Qwen3-4B decode at 16 threads 10 tok/s, 12 threads 20 tok/s. The cliff appears
at the point where the low-power cores enter the pool).

The engine applies the following defaults on the first model load:

- If `CANDLE_NUM_THREADS` is already set, it is respected as is (the default is not applied)
- If unset and there are 12 or more physical cores, it is set to `physical cores - 4`. Because decode is memory-bandwidth
  bound and saturates below the core count, the loss from this cap is small on a homogeneous many-core CPU and the
  straggler penalty disappears on a hybrid CPU
- Below 12 physical cores it is left untouched

The prefill path (the f32 matmul of the hybrid GEMM, blocked attention) uses a separate rayon pool
(`RAYON_NUM_THREADS`, default = all physical cores), so it is unaffected by this policy.
Thread count only changes the work partition, and the per-output-element computation is the same, so output under the same
seed is identical regardless of thread count.

---

## 9. HTTP API (OpenAI-compatible)

Start it with the `serve` command. The default binding is `127.0.0.1:8080` (local only; external exposure is the user's responsibility).

| Path | Method | Role |
|---|---|---|
| `/health` | GET | Process liveness check |
| `/v1/models` | GET | List of registered models (OpenAI list format) |
| `/v1/models/{id}` | GET | Check that a model exists |
| `/v1/completions` | POST | Prompt completion (non-streaming) |
| `/v1/chat/completions` | POST | Chat completion: SSE token streaming when `stream: true` |

### Chat completion

```bash
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-4b",
    "messages": [
      {"role": "system", "content": "You are a concise assistant."},
      {"role": "user", "content": "What is the capital of France?"}
    ],
    "max_tokens": 64,
    "temperature": 0.0,
    "stop": ["\n\n"]
  }'
```

The response is in OpenAI `chat.completion` format: `choices[0].message.content`, `finish_reason` (`stop` | `length`), and a `usage` token count.

### SSE streaming

With `"stream": true`, it streams OpenAI `chat.completion.chunk` as `text/event-stream`.
The first chunk carries the role, later chunks carry `delta.content`, the last chunk carries `finish_reason`, and the terminator is `data: [DONE]`.
If the client disconnects, the server stops decoding at the next token boundary (no wasted computation).

```bash
curl -sN http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "qwen3-4b", "messages": [{"role": "user", "content": "Count to 5."}], "stream": true}'
```

### Request parameters

| Parameter | Type | Default | Notes |
|---|---|---|---|
| `model` | string | required | a registered model id |
| `messages` / `prompt` | array / string | required | chat / completion |
| `max_tokens` | int | 256 | |
| `temperature` | float | 0.7 | 0.0 = deterministic |
| `top_k` | int | 40 | extension beyond the OpenAI standard |
| `seed` | int | 0 | fixed-seed reproduction |
| `stop` | string or array | none | stop string. The stop token is always applied separately |
| `stream` | bool | false | chat completion only |

---

## 10. CLI Tools

The binary name is `rust-ai-serving-engine`, and the model store is `--store <folder>` (default `.rust_ai_serving_engine`).

| Command | Arguments | Action |
|---|---|---|
| `model import` | `<path>` `--id` `[--kind]` `[--architecture]` `[--context-length]` `[--chat-template]` | Register a local GGUF/Safetensors with its hash |
| `model pull` | `--repo --file --id` `[--architecture]` `[--chat-template]` `[--tokenizer-repo --tokenizer-file]` | Download from Hugging Face and register. Given the tokenizer options, it downloads and links automatically |
| `model attach-tokenizer` | `<id>` `--tokenizer <path>` | Link a local tokenizer.json to a registered model |
| `model list` | | List registered models |
| `model inspect` | `<id>` | Print the manifest TOML |
| `model verify` | `<id>` | Re-verify the weight and tokenizer hashes |
| `runtime probe` | `[--device auto\|cpu\|cuda\|metal]` | Device selection + a real tensor-op smoke test |
| `serve` | `[--host]` `[--port]` `[--device]` | Start the OpenAI-compatible API server |

```bash
# Register a local file then verify integrity
rust-ai-serving-engine model import ./my-model.gguf --id my-model --architecture llama3
rust-ai-serving-engine model verify my-model

# Check the device
rust-ai-serving-engine runtime probe --device auto
```

---

## 11. Python Binding (PyO3)

Built with **abi3 (stable ABI)**, so it is compatible with Python 3.9 and up as a single wheel. The module name is `rust_ai_serving_engine`.

### Installation

```bash
# After publishing to PyPI: no Rust toolchain needed
pip install rust_ai_serving_engine

# From source (latest main / before publishing): the install machine needs a Rust toolchain
pip install "git+https://github.com/arabangoo/rust_ai_serving_engine"
```

### API

```python
import rust_ai_serving_engine as engine

engine.__version__                                  # e.g. "0.1.6"
engine.probe_runtime(device="auto")                 # device selection + tensor smoke test

# Model lifecycle (store = the model store folder)
engine.pull_model(store, repo, file, id, kind="generator",
                  architecture=None, context_length=None, chat_template=None)
engine.import_model(store, path, id, ...)           # register a local file (same arguments)
engine.attach_tokenizer(store, id, tokenizer_path)
engine.list_models(store)                           # ["qwen3-4b", ...]
engine.inspect_model(store, id)                     # manifest TOML string
engine.verify_model(store, id)                      # re-verify hashes
engine.unload_models()                              # free the entire process cache

# Generation: registered model (resident in the process cache, stop token automatic)
engine.generate_registered_gguf(store, id, prompt, max_tokens=256,
                                temperature=0.7, top_k=40, seed=0,
                                stop_tokens=[], device="auto")

# Chat generation: template applied automatically
engine.generate_chat_registered_gguf(store, id,
    [{"role": "user", "content": "..."}], max_tokens=256, ...)

# Chat streaming: the callback is called per text fragment. If it returns False, generation
# stops and the partial text so far is returned (other return values such as None continue).
engine.generate_chat_stream_registered_gguf(store, id, messages, on_delta,
                                            max_tokens=256, ...)

# Generation: direct file specification (one-off, without the registry)
engine.generate_llama_gguf(weights_path, tokenizer_path, prompt, ...)

# Performance profiling: forward-pass phase counters (see the "Performance profiling" section below)
engine.profiling_snapshot(reset=True)               # JSON string

# wgpu prefill offload status (section 8 wgpu): "active: <adapter>" | "inactive"
engine.gpu_info()
```

### Performance profiling (RASE_PROFILE)

A diagnostic surface that aggregates the per-phase time of the forward pass in nanosecond counters.
It decomposes prefill into "quantized linear GEMM work" and "attention kernel work", and decode into "quantized matvec"
and "fused attention", so you can judge numerically, before writing any code, the upper bound of what kernel optimization
or GPU offload would gain.

- **How to enable**: set the environment variable `RASE_PROFILE=1` before the process starts. It is read only once per
  process, so changing it during a run has no effect. When off (the default) it does not even set up the timers, so the
  inference path cost is zero, and measurement does not affect output (the same seed gives the same output whether on or off).
- **How to read**: `profiling_snapshot(reset=True)` returns all counters as a JSON string.
  `reset=True` zeroes them after reading, so the gap between successive calls is the measurement window.
- **Scope**: the CPU path of the Qwen3 GGUF decoder. On other decoders and devices the counters stay at 0.

| Counter | Meaning |
|---|---|
| `prefill_calls` / `prefill_tokens` | number of forward calls with sequence length 2 or more / number of prompt tokens processed |
| `prefill_forward_ns` | total wall-clock of prefill forward (from embedding to logits) |
| `gemm_dequant_ns` / `gemm_matmul_ns` | dequant / f32 matmul time of the hybrid GEMM path |
| `attn_blocked_ns` / `attn_flash_ns` | prefill attention kernel time (blocked / candle flash) |
| `decode_steps` / `decode_forward_ns` | number of single-token forwards / wall-clock (for computing decode tok/s) |
| `decode_matvec_ns` / `decode_attn_ns` | decode quantized matvec / fused attention time |

```python
import json
import os

os.environ["RASE_PROFILE"] = "1"      # must be before the first inference
import rust_ai_serving_engine as engine

engine.generate_chat_registered_gguf("./models", "qwen3-4b", [...], max_tokens=256)
p = json.loads(engine.profiling_snapshot(reset=True))
prefill = p["prefill_forward_ns"] / 1e9
gemm = (p["gemm_dequant_ns"] + p["gemm_matmul_ns"]) / 1e9
attn = (p["attn_blocked_ns"] + p["attn_flash_ns"]) / 1e9
print(f"prefill {prefill:.1f}s = GEMM {gemm:.1f}s + attention {attn:.1f}s + etc")
print(f"decode {p['decode_steps'] / (p['decode_forward_ns'] / 1e9):.1f} tok/s")
```

Note: `decode_matvec_ns` counts all sequence-length-1 linear calls, so it also includes the last prefill lm_head call
(once per call). If decode is several hundred tokens, the error is under 1%.

### Streaming integration recipe

The callback is the raw API. If you need server-sent events (SSE) or a generator, wrap it with a thread and a queue:
the generation loop runs with the GIL released and grabs the GIL only at the moment of the callback, so it runs
naturally alongside the host service.

```python
import queue
import threading

def stream_chat(messages):
    """A generator that yields token fragments in order (wires straight into FastAPI StreamingResponse etc.)."""
    q: queue.Queue = queue.Queue()
    done = object()

    def worker():
        try:
            engine.generate_chat_stream_registered_gguf(
                "./models", "qwen3-4b", messages,
                lambda delta: q.put(delta) or True,
            )
        finally:
            q.put(done)

    threading.Thread(target=worker, daemon=True).start()
    while (item := q.get()) is not done:
        yield item
```

### It does not stall the host service: GIL released

Long-running work such as download, hash verification, model load, and token generation all runs in Rust
**with the GIL (Global Interpreter Lock) released**. Even embedded in a host service such as FastAPI or Flask, other
request threads do not stall during generation (a Python heartbeat thread was confirmed to run normally during generation).

### Cache behavior

The first call of a registered-model generation function performs hash verification and load, and later calls reuse the
memory-resident model. If the manifest hash changes (the model file was replaced), it re-verifies and reloads automatically.
To reclaim memory, call `unload_models()`.

---

## 12. Embedding into a Service Pipeline

This engine is not a standalone app but a **core dependency you embed wherever local inference is needed**.
Pick one of the surfaces below according to the host environment.

| Host | Surface | Integration method |
|---|---|---|
| Existing OpenAI client code | HTTP server | Just change `base_url` to local |
| Python service (FastAPI etc.) | Python module | In-process inference with no server |
| Rust service | crate | Use the registry + session cache directly |
| Other languages / batch / orchestration | CLI + HTTP | `serve` as a sidecar |

### 12.1 OpenAI SDK: a one-line base_url swap

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="unused")
out = client.chat.completions.create(
    model="qwen3-4b",
    messages=[{"role": "user", "content": "Summarize this: ..."}],
    stream=True,
)
for chunk in out:
    print(chunk.choices[0].delta.content or "", end="")
```

LangChain is the same way: `ChatOpenAI(base_url="http://127.0.0.1:8080/v1", model="qwen3-4b")`.

### 12.2 In-process embedding into a Python service

Run inference directly inside the service without a separate server process. Because the GIL is released,
wrap it with the event loop's `run_in_executor` (or FastAPI's thread pool).

```python
import asyncio
import rust_ai_serving_engine as engine

STORE = "./models"

async def answer(messages: list[dict]) -> str:
    loop = asyncio.get_running_loop()
    return await loop.run_in_executor(
        None,
        lambda: engine.generate_chat_registered_gguf(STORE, "qwen3-4b", messages, max_tokens=256),
    )
```

### 12.3 Embedding into a Rust service

Generation is synchronous and CPU bound, so in an async server (axum etc.) wrap it with `spawn_blocking`.
Sharing `SessionCache` via `Arc` loads the model only once in the process.

```rust
use std::sync::Arc;
use rust_ai_serving_engine_core::{DevicePreference, GenerationConfig, ModelRegistry, generate};
use rust_ai_serving_engine_models::SessionCache;

// Once at startup
let cache = Arc::new(SessionCache::new());

// Handler
let cache = cache.clone();
let text = tokio::task::spawn_blocking(move || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let registry = ModelRegistry::open("./models")?;
    let session = cache.get_or_load(&registry, "qwen3-4b", DevicePreference::Auto)?;
    let mut session = session.lock().unwrap();
    let tokens = session.tokenizer.encode("The capital of France is", true)?;
    let mut config = GenerationConfig::default();
    if let Some(eos) = session.eos_token { config.stop_tokens.push(eos); }
    let out = generate(session.decoder.as_mut(), &tokens, &config, || false)?;
    Ok(session.tokenizer.decode(&out.tokens, true)?)
}).await??;
```

If you need a ready-made HTTP surface, you can also compose `rust_ai_serving_engine_api::{router, serve, ApiState}`
straight into your own server.

### 12.4 Other languages / batch: sidecar

In Java, Node, Go, and so on, the simplest approach is to run `serve` as a sidecar process and connect with an OpenAI client.
Being a single binary, you only need to put one executable + a model folder into the container.

---

## 13. Adding a New Model Architecture

A new GGUF architecture attaches in three steps. You do not touch the core generation loop, the API, or the CLI.

1. **Implement the decoder**: implement `TokenDecoder`. Wrapping Candle's quantized model implementation is the basic form.

```rust
use rust_ai_serving_engine_core::{Result, TokenDecoder};

pub struct MyArchDecoder { /* ModelWeights + device + position */ }

impl TokenDecoder for MyArchDecoder {
    fn prefill(&mut self, prompt: &[u32]) -> Result<Vec<f32>> {
        // init the KV cache -> forward the whole prompt -> logits at the last position (rank-1 Vec<f32>)
    }
    fn decode(&mut self, token: u32) -> Result<Vec<f32>> {
        // forward one token (accumulate the KV cache) -> next logits
    }
    fn eos_token(&self) -> Option<u32> { /* the value read from the GGUF metadata */ }
}
```

2. **Register the architecture mapping**: add the architecture name to the match in `load_gguf_decoder`.
   The convention is to refuse an unsupported combination with `UnsupportedArchitecture` instead of producing wrong output.

3. **Wire the chat template**: if the existing three are enough, add only a default to `ChatTemplate::for_architecture`,
   and if new markup is needed, add a variant and a render function (rendering is a pure function, so it is pinned by unit tests).

Two contracts to watch: `prefill` must initialize the KV cache (to prevent contamination from the previous conversation),
and logits must be returned as a **rank-1 vector** (if Candle forward gives `(batch, vocab)` rank-2, a squeeze is needed;
this missing step was in fact a fatal bug, caught by a real-model end-to-end test).

---

## 14. Build, Features, and Tests

If you clone this repository, you must build it once with a **Rust toolchain (stable)**.

| Usage | Build command | Output |
|---|---|---|
| CLI + server | `cargo build --release` | `target/release/rust-ai-serving-engine` single binary |
| Python module | `pip install maturin && maturin develop --release` | `import rust_ai_serving_engine` in the current venv |
| Rust library | a `git`/`path` dependency in `Cargo.toml` | linked into another Rust project |

```bash
# Build and test the whole workspace
cargo build --release
cargo test --workspace
cargo clippy --all-targets

# Confirm the Python extension gate compiles
cargo check -p rust-ai-serving-engine-python --features python

# Build the distribution wheel
maturin build --release          # abi3 wheel in dist/
```

The tests deterministically verify the range that works without a model file: the generation loop (stop, cancel), the registry
(registration, hash-tamper detection, tokenizer linking), the three chat-template renders, and stop-string parsing and hold-back boundaries.

### Real-model smoke (manual)

After a code change, a real-model regression follows the CLI flow of [Section 2](#2-quick-start) exactly: pull a small GGUF,
start `serve`, and call chat completion (non-streaming and streaming). With `temperature: 0.0` + a fixed `seed`, also confirm
output stability for the same input.

---

## 15. Directory Structure

```text
rust_ai_serving_engine/
  Cargo.toml                              # workspace definition
  pyproject.toml                          # maturin build metadata (PyPI package)
  README.md                               # this document
  crates/
    rust_ai_serving_engine_core/
      src/
        lib.rs                            # crate root, re-exports
        manifest.rs                       # ModelManifest / ModelKind / ModelFormat
        registry.rs                       # ModelRegistry (register, hash, verify, atomic write)
        generation.rs                     # TokenDecoder / GenerationConfig / generate(_with) / sampler
        runtime.rs                        # DevicePreference / RuntimeDevice (CPU, CUDA, Metal)
        hub.rs                            # HuggingFaceHub download
        error.rs                          # EngineError
    rust_ai_serving_engine_models/
      src/
        lib.rs                            # load_gguf_decoder, GGUF EOS extraction
        llama_gguf.rs                     # Llama/Mistral GGUF decoder
        qwen3_gguf.rs                     # Qwen3 GGUF decoder
        qwen3_model.rs                    # Qwen3 forward (hybrid prefill GEMM + blocked attention)
        profiling.rs                      # RASE_PROFILE phase counters (section 11 performance profiling)
        threading.rs                      # decode thread default policy (section 8 decode thread policy)
        gpu_gemm.rs                       # wgpu prefill GEMM offload (section 8, opt-in via RASE_GPU=1)
        chat.rs                           # ChatTemplate (ChatML, Llama3, Mistral) + render tests
        session.rs                        # ModelSession / SessionCache
        tokenizer.rs                      # LocalTokenizer (tokenizer.json)
    rust_ai_serving_engine_api/
      src/lib.rs                          # OpenAI-compatible HTTP API + SSE streaming
    rust_ai_serving_engine_cli/
      src/main.rs                         # model / runtime / serve commands
    rust_ai_serving_engine_python/
      src/
        lib.rs                            # feature gate
        python.rs                         # PyO3 binding (GIL released + process session cache)
```

---

## 16. License and Model Responsibility

The engine code is Apache-2.0.

The licenses of the model weights, tokenizer, and GGUF conversions are separate from the engine. For each model registered in
the registry, the user must confirm the source and license terms (including whether redistribution is allowed), and for
commercial distribution include it in a bundle only after confirming the per-model terms.
