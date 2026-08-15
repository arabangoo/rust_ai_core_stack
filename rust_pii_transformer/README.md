# rust_pii_transformer

> **Deterministic Korean PII detection and masking, written in Rust.**
>
> It finds personally identifiable information in Korean text and masks it with a **guaranteed byte-exact
> recovery of the original**, using **no model files at all**.

Zero model files, zero GPU, zero external API calls, zero network access. Once installed it runs on an
air-gapped machine. The default build has exactly two dependencies, `thiserror` and `serde`.

**The differentiator is variant-notation recognition.** Beyond plain digit strings, the normalization layer
absorbs Korean numeral words (`팔팔공일공일`), separator and spacing variants (`880101 - 1234567`),
full-width digits (`８８０１０１`), decomposed Hangul jamo, letters that look like digits (`88O1O1`),
already-partially-masked values (`880101-1******`), and the trailing verb endings that speech transcripts
leave behind (`구공오삼일삼이래요`). It then detects on the clean text and maps every hit **back to exact
offsets in the original**.

**This library is for Korean text only.** The entities are Korean national identifiers and the context
dictionaries are Korean words. Being narrow is the point. Instead of chasing what existing tools already do
well in English, it does **what those tools cannot do in Korean**. That said, you do not have to be a Korean
speaker to use it: every API identifier and JSON name is English, and masking placeholders can be English too.

This document is the **complete developer manual** for the library. It covers the design rationale, the
public API, per-entity behavior and maturity, normalization and scoring rules, masking policies, Python
usage, the build and test workflow, and **what this library cannot do**.

**Key references**

1. REDACT: A Systematically Controlled Multilingual Benchmark for Personal Information Detection (Vats et al., 2026) - https://arxiv.org/abs/2606.19881
2. Microsoft Presidio, whose five Korean recognizers (`KR_RRN` `KR_FRN` `KR_BRN` `KR_DRIVER_LICENSE` `KR_PASSPORT`) define the current baseline - https://presidio.dataprivacystack.org
3. ISO/IEC 7812, identification card numbering and the Luhn check digit, the only publicly standardized check among the entities handled here - https://www.iso.org/standard/70484.html
4. KFTC CMS account numbering scheme, the source for bank account length ranges - https://www.cmsedi.or.kr/cms/board/workdata

The first reference is both the reason this library exists and the honest limit on what it claims. REDACT
annotates over 324,000 entities across 25 languages and 51 types, and its conclusion is that **rule-based
detectors collapse on the highest-stakes categories while LLM detectors stay more robust.** The reasoning
behind choosing rules anyway is spelled out in [15. Known Limitations](#15-known-limitations) and
[7. Per-Entity Behavior and Maturity](#7-per-entity-behavior-and-maturity). It comes down to one thing:
concentrate on what rules catch reliably, and do not hide what they miss.

The second reference is the evidence for the gap this project fills. Presidio ships five Korean recognizers,
**none of them for bank account numbers**, and all of them assume plain digit strings, so every variant
listed above slips past.

The check-digit weights for resident and foreigner registration numbers, and how strong the evidence is for
the business registration number algorithm, are recorded per entity in the table in
[7. Per-Entity Behavior and Maturity](#7-per-entity-behavior-and-maturity). Where public documentation could
not be confirmed, that table says so.

## Measured Accuracy

Measured on a synthetic corpus of 3,160 samples (2,680 positive, 480 negative). The seed is fixed, so anyone
reproduces the same numbers.

| Axis | Value |
| --- | --- |
| Overall recall | **99.9%** (TP 2676 / FN 4) |
| Overall precision | **99.3%** (FP 20) |
| Masking round-trip failures | **0** (all 3,160 samples round-tripped) |
| False positive rate on negatives | 3.3% (16 of 480) |

By notation variant, Korean numerals, full-width digits, spacing, lookalike letters, partial masking, and
verb endings all reach 100%; plain digits 99.8%; hyphenated 99.4%. By entity, bank account numbers reach
95.0% and the other nine reach 100%. How to reproduce is in [16. Build and Test](#16-build-and-test).

```bash
cargo test --test accuracy -- --nocapture
```

## What Is Inside

| Layer | What it does |
| --- | --- |
| Offset mapping (`span`) | A monotone span alignment table linking text before and after normalization |
| Normalization (`normalize`) | Jamo composition, full-width folding, Korean numeral conversion, lookalike correction, separator absorption |
| Detection (`detect`) | Check digits, context scoring, evidence |
| Masking (`mask`) | Four policies and a restore map |
| Synthetic corpus (`synth`) | Samples with valid check digits, plus notation variants |
| Command-line tool (`rpit`) | `--features cli` |
| Python bindings | All four layers exposed. `--features python` |

---

## Table of Contents

1. [Key Features](#1-key-features)
2. [Quick Start](#2-quick-start)
3. [Installation and Cargo Features](#3-installation-and-cargo-features)
4. [Architecture](#4-architecture)
5. [Common Type Reference](#5-common-type-reference)
6. [Public API Reference](#6-public-api-reference)
7. [Per-Entity Behavior and Maturity](#7-per-entity-behavior-and-maturity)
8. [Normalization Behavior](#8-normalization-behavior)
9. [Scoring Rules and Configuration](#9-scoring-rules-and-configuration)
10. [Masking](#10-masking)
11. [Synthetic Validation Corpus](#11-synthetic-validation-corpus)
12. [Command-Line Tool rpit](#12-command-line-tool-rpit)
13. [Performance](#13-performance)
14. [Python Bindings](#14-python-bindings)
15. [Known Limitations](#15-known-limitations)
16. [Build and Test](#16-build-and-test)
17. [License](#17-license)

---

## 1. Key Features

| Feature | What it means |
| --- | --- |
| **Variant-notation recognition** | Korean numeral words, separator and spacing variants, full-width digits, decomposed jamo, digit-lookalike letters, already-masked values, and speech-transcript verb endings are all absorbed in the normalization layer |
| **Guaranteed original recovery** | The reverse span alignment table is built during normalization. Every detected span maps back to an exact byte range in the original |
| **Evidence on every hit** | Which pattern matched, which check digit passed, which context cues were found, and which normalizations applied all ship with the result |
| **Reasons for every miss** | Candidates that matched the shape but failed scoring come back with a reason, so "why was this not flagged" is answerable |
| **No models** | Zero model files, zero GPU, zero external APIs, zero network |
| **Deterministic** | The same input always produces the same output, which makes caching, testing, and audit trails possible |
| **Self-contained** | Pure Rust. Zero FFI, zero subprocess calls |

---

## 2. Quick Start

### Detection

```rust
use rust_pii_transformer::detect::{detect, Config, Certainty, EntityKind};

let text = "주민등록번호 팔팔공일공일 - 1234567 입니다";
let report = detect(text, &Config::default()).unwrap();

let finding = &report.findings[0];
assert_eq!(finding.entity, EntityKind::Resident);

// The leading digits, spelled out as Korean numerals, map back to the exact source range.
assert_eq!(finding.source.slice(text), "팔팔공일공일 - 1234567");

// Evidence ships with the result.
assert_eq!(finding.evidence.cost.expanded_syllables, 6);   // syllables expanded into digits
assert_eq!(finding.evidence.cost.absorbed_whitespace, 2);  // whitespace absorbed
assert!(finding.certainty >= Certainty::Probable);

// Misses are reported too.
for rejection in &report.rejections {
    println!("{:?} {:?} {:.2}", rejection.entity, rejection.reason, rejection.score);
}
```

### Using Normalization Alone

The normalization layer works without the detector, which is useful as a preprocessing stage in front of a
different detection engine.

```rust
use rust_pii_transformer::{normalize, NormalizeConfig, Span};

let out = normalize("팔팔공일공일 - 1234567", &NormalizeConfig::default()).unwrap();
assert_eq!(out.text, "8801011234567");

// Map a span on the normalized text back to source coordinates.
// Six Hangul syllables at 18 bytes + 3 separator bytes + 7 digit bytes = 28 bytes.
let src = out.map.to_source(&Span::new(0..13, 0..13));
assert_eq!(src.span.byte, 0..28);
assert!(src.rules.contains(&"hangul.digit_reading"));
```

### Runnable Example

```bash
cargo run --example demo
```

It runs over a document shaped like a customer support record and prints what was caught, what was not, and
the reason each rejected candidate was dropped.

---

## 3. Installation and Cargo Features

Not published yet, so pull it from source.

```toml
[dependencies]
rust_pii_transformer = { git = "https://github.com/arabangoo/rust_pii_transformer" }
```

```toml
[features]
default = []                             # core. pure Rust, zero FFI
hash    = ["dep:sha2", "dep:hmac"]       # hash pseudonymization masking policy
cli     = ["dep:clap", "dep:serde_json"] # rpit command-line tool
python  = ["dep:pyo3", "dep:serde_json"] # PyO3 abi3 extension module
```

**The core includes offset mapping, normalization, detection, masking, and the synthetic corpus.** Only
those three are opt-in.

| Dependency | When | Purpose |
| --- | --- | --- |
| `thiserror` 2 | always | error types |
| `serde` 1 | always | serializing results |
| `sha2` 0.10, `hmac` 0.12 | `hash` | hash pseudonymization policy |
| `clap` 4 | `cli` | command-line argument parsing |
| `pyo3` 0.29 | `python` | Python extension module |
| `serde_json` 1 | `cli`, `python` | JSON output |

The synthetic corpus uses a hand-written linear congruential generator instead of a random-number crate for
a reason. A corpus has to be **reproducible**, which calls for a deterministic generator with a fixed seed,
not statistical randomness quality. That choice keeps the feature dependency-free and in the default build.

Hash pseudonymization is the one thing behind a flag, which keeps the promise of a two-dependency default
build. RustCrypto is pure Rust so FFI is still zero, but rather than reimplement a primitive, the library
uses a vetted one and charges the cost only to whoever turns it on.

### Minimum Supported Rust Version

The default build needs **1.71** (`thiserror` 2 sets the bar). `--features python` needs **1.83** because of
PyO3 0.29. To avoid raising the floor for every core consumer over one opt-in path, `package.rust-version`
stays at 1.71.

---

## 4. Architecture

A three-stage pipeline. Each stage is independently testable and independently usable.

```text
source text
   |
   v
[stage 1] normalize  nfc -> fold -> hangul -> lookalike -> separator
   |  two outputs: normalized text, SpanMap (reverse span alignment table)
   v
[stage 2] detect     digit-run scan -> entity candidates -> check digits -> context score -> certainty
   |  outputs: Finding list + Rejection list
   |  SpanMap.to_source() recovers source spans (with boundary snapping)
   v
[stage 3] mask       apply policy (redact / partial / hash / tokenize)
   |  outputs: masked text + restore map (for tokenize)
   v
bytes outside the detected spans are identical to the source
```

The important part is that **`SpanMap` sits between stage 1 and stage 2**. Detection runs on clean
normalized text and masking runs on the original, and since `SpanMap` is the only thing joining them,
**the responsibility for recovery correctness lives in exactly one place.**

### The Problem Offset Mapping Solves

Normalization deletes characters (absorbing hyphens), substitutes them (full-width to half-width), grows
them (`일억` becomes nine digits), and shrinks them (`구십일` becomes two). So there is no one-to-one
correspondence between source and normalized text. **Order, however, is always preserved.** The segment
table is built on that monotonicity.

A segment table beats a per-character index array for three reasons. Identity runs fold into a single entry,
so real text yields tens of entries rather than thousands; lookups are a binary search; and each entry can
carry **which rule produced this range**. Evidence output falls out of that for free.

### Both Byte and Character Offsets Are Stored

Rust string slicing uses byte indices while Python and JavaScript consumers use character indices. A Korean
syllable is three UTF-8 bytes, so the two never agree. Converting later costs an O(n) scan every time and
becomes a fresh source of offset bugs, so both live in the segment from the start.

### Verification Harness

Four invariants pin down correctness.

| Invariant | Statement | Status |
| --- | --- | --- |
| **Coverage** | Every offset belongs to exactly one segment. Concatenating segments reconstructs both texts byte for byte | Verified |
| **Composition associativity** | `compose(compose(a, b), c)` equals `compose(a, compose(b, c))` | Verified |
| **Round-trip** | Mapping a detected span back to the source and re-normalizing that fragment reproduces the original range | Verified on the corpus |
| **Lossless masking** | Bytes outside the detected spans are identical to the source | Verified. 0 failures across 3,160 corpus samples |

The randomized test runs 400 rounds off a fixed-seed linear congruential generator. No external
property-testing crate is used, because the zero-dependency rule applies to dev-dependencies too, and the
fixed seed means any failure reproduces exactly.

---

## 5. Common Type Reference

### Spans and Mapping

```rust
pub struct Span {
    pub byte: Range<u32>,   // for Rust slicing
    pub char: Range<u32>,   // for Python and JavaScript consumers
}

pub struct SourceSpan {
    pub span: Span,                  // span in source coordinates
    pub snapped: bool,               // was it widened past a segment boundary
    pub rules: Vec<&'static str>,    // normalization rules applied, sorted by name
    pub cost: NormalizationCost,     // basis for the confidence penalty
}

pub struct NormalizationCost {
    pub absorbed_whitespace: u16,    // whitespace absorbed. the riskiest kind
    pub absorbed_separators: u16,    // separators absorbed
    pub expanded_syllables: u16,     // Hangul syllables expanded into digits
    pub replaced_chars: u16,         // characters substituted
}

pub struct Segment {
    pub src: Span,                   // range on the source side
    pub dst: Span,                   // range on the normalized side
    pub kind: SegmentKind,           // Identity | Replace | Delete | Expand | Insert
    pub rules: Vec<&'static str>,
    pub cost: NormalizationCost,
}
```

Offsets are `u32` for two reasons. A single text over four gigabytes is not what this library is for, and a
segment carries eight offsets, so halving their size is a direct memory saving.

`rules` is normalized to **ascending name order**, not application order. Leaving application order in place
would make output depend on how the pipeline happened to be assembled, which breaks determinism. What
evidence needs is the set of rules involved, not their sequence.

### Detection Results

```rust
pub struct Report {
    pub findings: Vec<Finding>,        // accepted results, ascending by source position
    pub rejections: Vec<Rejection>,    // dropped candidates and why
    pub normalized_text: String,       // for inspecting evidence
}

pub struct Finding {
    pub entity: EntityKind,
    pub source: Span,          // source coordinates. masking applies here
    pub normalized: Span,      // normalized coordinates
    pub certainty: Certainty,  // Possible < Probable < Certain
    pub score: f32,
    pub evidence: Evidence,
}

pub struct Evidence {
    pub rule: &'static str,                  // name of the pattern rule that matched
    pub checksum: ChecksumResult,            // Passed | Failed | NotApplicable(reason)
    pub context_hits: Vec<ContextHit>,       // cues found and their distances
    pub normalizations: Vec<&'static str>,   // normalization rules applied
    pub cost: NormalizationCost,             // basis for the penalty
    pub snapped: bool,
}

pub struct ContextHit {
    pub cue: &'static str,   // the word that matched
    pub distance: u32,       // character distance from the span boundary
    pub weight: f32,         // weight after distance decay
}

pub struct Rejection {
    pub entity: EntityKind,
    pub source: Span,
    pub reason: RejectReason,  // ChecksumFailed | NoContext | BelowThreshold | Outranked | BusinessContext
    pub score: f32,
}
```

`EntityKind` has ten variants: `Resident`, `ForeignerRegistration`, `BusinessRegistration`, `CreditCard`,
`BankAccount`, `Phone`, `Email`, `DriverLicense`, `BirthDate`, `Passport`. `label()` returns the Korean name.

`Finding` implements `serde` serialization, so it goes straight to JSON. **Deserialization is deliberately
not supported.** Rule identifiers are `&'static str` to eliminate heap allocation, and deserializing that
would require the input to live for `'static`, which cannot be arranged. This library exists to emit
verdicts, not to read them back.

---

## 6. Public API Reference

### Detection (`detect`)

```rust
pub fn detect(text: &str, cfg: &Config) -> Result<Report>;

pub struct Config {
    pub normalize: NormalizeConfig,
    pub context_window: u32,   // window for context cues, in characters. default 24
    pub min_score: f32,        // below this, nothing is reported. default 0.5
    pub min_context: f32,      // minimum context total for context-required entities. default 0.3
    pub min_veto: f32,         // minimum negative total before a veto can apply. default 0.5
    pub weights: Weights,
}
```

### Normalization (`normalize`)

```rust
pub fn normalize(text: &str, cfg: &NormalizeConfig) -> Result<Normalized>;

pub struct Normalized {
    pub text: String,
    pub map: SpanMap,
}

pub struct NormalizeConfig {
    pub nfc: bool,             // compose Hangul jamo
    pub fold: bool,            // fold full-width forms
    pub hangul: bool,          // convert Korean numeral words to digits
    pub lookalike: bool,       // correct digit-lookalike letters
    pub separator: bool,       // absorb separators
    pub numeral: NumeralConfig,
}

impl NormalizeConfig {
    /// Only the passes that do not depend on context (nfc, fold).
    pub fn context_free() -> Self;
}
```

Passes can be turned off individually. In accounting documents full of decimal points, the `separator` pass
turns `1.5` into `15` and that becomes a source of false positives, so that one pass can be disabled.

### Offset Mapping (`span`)

```rust
impl SpanMap {
    pub fn identity(text: &str) -> Self;
    pub fn compose(inner: &SpanMap, outer: &SpanMap) -> Result<SpanMap>;
    pub fn to_source(&self, dst: &Span) -> SourceSpan;
    pub fn validate(&self) -> Result<()>;
    pub fn segments(&self) -> &[Segment];
    pub fn is_identity(&self) -> bool;
}

impl SpanMapBuilder {
    pub fn keep(&mut self, text: &str);                                    // pass through
    pub fn replace(&mut self, src: &str, dst: &str, rule: &'static str);   // character folding
    pub fn numeral(&mut self, src: &str, dst: &str, rule: &'static str);   // Korean numerals
    pub fn absorb(&mut self, src: &str, rule: &'static str, class: Absorbed); // separator absorption
    pub fn finish(self) -> (String, SpanMap);
}
```

To write your own normalization pass, feed fragments through `SpanMapBuilder` and join the result onto the
existing pipeline with `SpanMap::compose`. Each method fills in its own cost field automatically, so a pass
author cannot forget to account for one.

### Check Digits (`detect::checksum`)

```rust
pub fn analyze_resident(digits: &[u8]) -> Option<ResidentAnalysis>;  // resident and foreigner alike
pub fn business_registration(digits: &[u8]) -> ChecksumResult;
pub fn luhn(digits: &[u8]) -> ChecksumResult;
pub fn gender_code(digit: u8) -> Option<GenderCode>;
pub fn is_valid_date(year: u16, month: u8, day: u8) -> bool;
pub fn to_digits(text: &str) -> Option<Vec<u8>>;
```

The check-digit functions are usable on their own. This module contains **only calculations whose basis was
confirmed**. That there is no function for driver's license numbers is that principle in action.

### Scanner and Context (`detect::scanner`, `detect::context`)

```rust
pub fn scanner::digit_runs(text: &str) -> Vec<Span>;
pub fn scanner::emails(text: &str) -> Vec<Span>;
pub fn context::find(text: &str, span: &Span, window: u32, cues: &[Cue]) -> Vec<ContextHit>;
pub fn context::total(hits: &[ContextHit]) -> f32;
```

Context dictionaries are exposed per entity as constants (`context::RESIDENT`, `context::ACCOUNT`, and so on).

### Errors

```rust
pub enum Error {
    SpanMapInvariant { index: usize, detail: String },
    SpanMapMismatch { detail: String },
}
```

Neither is reachable from user input. Both mean a normalization pass has a bug, so seeing one means that
pass needs fixing.

---

## 7. Per-Entity Behavior and Maturity

Every verification method was implemented **only after confirming its basis**. How strong that basis is
appears in the last column.

| Entity | Verification | Ceiling | Strength of basis |
| --- | --- | --- | --- |
| Resident registration number | date validity + gender code + check digit | `Certain` | **Confirmed.** Weights `2,3,4,5,6,7,8,9,2,3,4,5`, `(11 - sum mod 11) mod 10` |
| Foreigner registration number | same structure, gender codes 5 through 8 | `Certain` | **Confirmed.** Same weights with a `+2` correction |
| Business registration number | check digit | `Certain` | **Partially confirmed.** Official documentation does not publish the algorithm. Confirmed empirically |
| Credit card number | Luhn | `Certain` | **Confirmed.** The only check published as an international standard (ISO/IEC 7812) |
| Phone number | format (mobile, Seoul, regional, service numbers) | `Probable` | No check digit exists |
| Email | structure | `Probable` | No check digit exists |
| Driver's license number | 12-digit format + region code | `Probable` | A check digit exists but **its formula is unpublished** |
| Bank account number | length (10 to 14) + context | `Probable` | Schemes differ by bank, so no common check exists. Context is required |
| Date of birth | date validity + context | `Probable` | No check digit exists. Context is required |
| Passport number | format (1 to 2 letters + 7 to 8 digits) + context | `Probable` | No published check exists. Context is required |

**The ceiling column is the important one.** An entity that cannot reach `Certain` cannot have its false
positives fully eliminated, structurally. That fact is pinned into the type system via `Candidate::ceiling`,
so no amount of context can push a verdict past its ceiling.

### A Failed Check Is Not Grounds for Discarding

This call matters most for resident registration numbers. **The October 2020 reform made the trailing six
digits arbitrary, so the check digit does not hold for numbers issued after that date.** Nothing in the
number itself reveals when it was issued.

So a failed check is never grounds for dropping a candidate, only for **lowering its certainty**. A number
that fails the check but has a valid date and strong context survives as `Probable`. Conversely, if the
leading six digits are not a real date, the candidate is dropped regardless of the check digit, because
date validity is a constraint the reform did not touch.

### When One Range Yields Several Candidates

A ten-digit run could be a business registration number, a regional phone number, or a bank account number.
All candidates are evaluated, **only the highest-scoring one is reported**, and the rest are recorded in
`rejections` with the `Outranked` reason. Why the other verdicts did not win is on the record.

Overlapping ranges from different scanners are resolved the same way. The passport number `M12345678`
contains an eight-digit run, so two scanners each claim the same position. The wider one already explains
the narrower one, so the narrow one is dropped.

---

## 8. Normalization Behavior

Passes run in order and each produces its own `SpanMap`.

| Order | Pass | What it does | Rule names |
| --- | --- | --- | --- |
| 1 | `nfc` | Composes decomposed Hangul jamo into syllables | `nfc.hangul_jamo` |
| 2 | `fold` | Full-width to half-width, and unifies dash and space variants | `fold.fullwidth`, `fold.dash`, `fold.space` |
| 3 | `hangul` | Converts Korean numeral words back to digits | `hangul.digit_reading`, `hangul.unit_reading` |
| 4 | `lookalike` | Converts digit-lookalike letters wedged between digits | `lookalike.digit` |
| 5 | `separator` | Absorbs hyphens, dots, spaces, and brackets between digits | `separator.hyphen`, `separator.whitespace`, and others |

The order is deliberate. Jamo must be composed first for numeral syllables to be visible as syllables;
full-width forms must be folded first for full-width digits to read as digits; numerals must become digits
first for the "digits on both sides" condition of separator absorption to hold. Lookalike correction sits
before separator absorption for the mirror image of that reason. That pass only substitutes when both
neighbors are digit positions, and removing separators first would erase the evidence in real notations like
`88-O1-O1` where separators and lookalikes are interleaved. So it looks while separators are still present,
and skips over them while looking.

### Conversion Examples

| Input | Normalized |
| --- | --- |
| `팔팔공일공일` | `880101` |
| `구십일년` | `91년` |
| `천구백팔십팔년생` | `1988년생` |
| `８８０１０１－１２３４５６７` | `8801011234567` |
| `880101 - 1234567` | `8801011234567` |
| `88O1O1` | `880101` |
| `POLO 매장` | `POLO 매장` (left alone) |
| `구공오삼일삼이래요` | `905313이래요` |
| `제품 1234 수량 5678` | `제품 1234 수량 5678` (not joined) |
| `이사 갑니다. 만원만 빌려줘` | `이사 갑니다. 만원만 빌려줘` (left alone) |

### Two Reading Grammars

Korean numeral notation has two grammars, which is why a single lookup table is not enough.

- **Digit-by-digit reading**: one syllable per digit. `팔팔공일공일` becomes `880101`, `공일공` becomes `010`
- **Positional reading**: tens, hundreds, thousands, ten-thousands, hundred-millions are computed.
  `구십일` becomes `91`, `이천이십사` becomes `2024`

The discriminator is simple. **If the run contains any positional syllable it is positional reading,
otherwise it is digit-by-digit.**

### False Positive Gates

Numeral syllables collide with ordinary words constantly, as in `사구`, `이사`, `구이`. Four gates guard
against that.

- **Minimum digit count.** If the **result** has at least 6 digits (default), it applies without context.
  Measuring the result rather than the syllable count matters because the two grammars differ in syllable
  count: `구십일` is three syllables but yields two digits
- **Adjacent numeric context.** Anything shorter applies only when a numeric marker (`년`, `월`, `일`, `번`,
  `호`, `생`, `세`, `원`, `차`, `기`, `시`, `분`, `초`), an adjacent digit, or a hyphen sits on either side
- **An extra condition for positional reading.** A run with no digit syllable at all is not converted, so
  `만원` never becomes `10000원`
- **Deferral to downstream verification.** Passing these gates is not a verdict; check digits and date
  validity decide

Ungrammatical positional notation, such as `이삼십` where two or more digits precede a unit, is abandoned and
left as-is. The same applies when carrying would overflow 64 bits.

### Verb Endings Are Not Swallowed Into Values

This one comes from speech-to-text input. In `구공오삼일삼이래요` the `이` is the digit 2 and `구` is 9, so a
greedy numeral run eats the first syllable of the verb ending and produces `9053132`. One digit too many, and
it no longer reads as the leading half of a resident registration number.

So after collecting a run, the pass **checks whether a verb ending was swallowed at the end and shortens the
run accordingly.** The pairs are (`이`, `래요`), (`이`, `에요`), (`이`, `예요`), (`이`, `요`), (`이`, `고`),
(`이구`, `요`). The removed syllable is not discarded; it stays in the source text and simply is not read as
a digit.

With no verb ending following, nothing is shortened. The final `이` in `구공오삼일삼이` is just the digit 2.

The three thresholds are adjustable through `NumeralConfig`.

```rust
pub struct NumeralConfig {
    pub min_digits_without_context: usize,  // default 6
    pub min_digits_with_context: usize,     // default 2
    pub context_window: usize,              // default 2 (how many spaces to skip)
}
```

### Separator Absorption Is Deliberately Narrow

Deleting whitespace globally would turn `제품 1234 수량 5678` into `제품12345678`, **manufacturing an
eight-digit run that never existed.** So absorption happens **only between two digits.** Risk remains even
then, so the count of absorbed whitespace is recorded in `NormalizationCost.absorbed_whitespace` and
subtracted from confidence. Hyphen absorption is far less risky and carries a different coefficient.

**Line breaks are never absorbed.** Different lines are likely different values. Only horizontal whitespace
(spaces and tabs) is eligible, which prevents the last cell of a table row from fusing with the first cell of
the next.

### What Normalization Does Not Do

The scope is deliberately narrow. Widening it damages ordinary text.

| Item | What is not done | Why |
| --- | --- | --- |
| Unicode composition | Decomposed forms of non-Hangul characters are not composed | That would require pulling in the full composition table as a dependency. Hangul composition is arithmetic and needs no table |
| Compatibility jamo | Standalone `ㄱ`, `ㄴ` at U+3131 and above are not composed | Standard NFC does not compose them either. They are distinct characters meaning the letter itself |
| Lookalike correction | Nothing is substituted unless both neighbors are digit positions. `S` versus `5` and `B` versus `8` are not handled at all | Unconditional substitution would turn `POLO` into `P0L0`. `S` and `B` are far too common in ordinary text |
| Decimal points | `1.5` becomes `15` | The phone-number payoff was judged worth it. It is counted as separator absorption and penalized, and the pass can be turned off |

### Boundary Snapping

A detected span can cut across a non-identity segment. That happens when `일억` expands to `100000000` and
only `100000` inside it matches. In that case the span is **widened out to the segment boundary to preserve
atomicity, and the fact that it widened is recorded as `snapped: true` in the evidence.** A widened span
still has to pass check-digit verification, so a bad expansion never reaches the final result.

---

## 9. Scoring Rules and Configuration

### Score

```text
score = pattern base
      + check-digit bonus (a penalty on failure)
      + context total x coefficient
      - normalization cost penalty
```

```rust
pub struct Weights {
    pub checksum_passed: f32,       // default 0.6
    pub checksum_failed: f32,       // default 0.25 (penalty)
    pub context: f32,               // default 0.5
    pub absorbed_whitespace: f32,   // default 0.08. the heaviest
    pub absorbed_separator: f32,    // default 0.01
    pub expanded_syllable: f32,     // default 0.01
    pub replaced_char: f32,         // default 0.002. lightest, since the value is unchanged
}
```

The penalties exist to quantify the price of the "normalize liberally, verify strictly" principle. A digit
run assembled by absorbing three spaces deserves less trust than one that was already contiguous.

### Certainty Levels

| Level | Condition |
| --- | --- |
| `Certain` | Check digit passed, and only if the entity's ceiling is `Certain` |
| `Probable` | Context cues were found, or the entity has no check digit |
| `Possible` | Shape matches only. No context cues |

`Certainty` implements `Ord`, so filtering with `certainty >= Certainty::Probable` works.

### Context Scoring

Cues are searched only inside the configured window around the span, and **weight decays with distance**
(`1 / (1 + distance/8)`). A `주민등록번호` sitting immediately before the value cannot count the same as one
two sentences away.

The total is **capped at 1.5**. Without a cap, a sentence containing `주민등록번호`, `주민번호`, and
`주민등록` at once would triple the score. Three words saying the same thing is not three times the evidence.

Cue lists are sorted by descending weight, then ascending word. Identical input has to produce
character-identical evidence.

### Entities That Require Context

Bank account numbers, driver's license numbers, dates of birth, and passport numbers say nothing on their
own. Whether `1234567890` is an account number or an order number is not knowable by rule. For these four, a
context total below `min_context` (default 0.3) drops the candidate with the `NoContext` reason.

### Negative Context

Every cue described so far pushes toward "this is PII", which turns entire business documents into false
positives. A single contract contains twenty contract, policy, and reference numbers, and the ten-digit ones
become business registration candidates while the eleven-digit ones become phone candidates.

`context::EXCLUDE` is a dictionary of words that say **this number does not belong to a person**. It covers
document and transaction identifiers (policy number, contract number, reference number, order number,
invoice number, tracking number), object identifiers (product code, model number, part number), phone
numbers that are not personal (call center, customer service, main line, ARS), and document structure
(postal code, footnote markers, notices). The lookup method and distance decay are identical to positive cues.

Three conditions must all hold before a candidate is vetoed.

| Condition | Why |
| --- | --- |
| The check digit did **not** pass | A thirteen-digit number with a valid check digit is PII even next to the word `증권번호`. The label is simply wrong |
| The negative total is at least `min_veto` (default 0.5) | One word grazing the window must not overturn a verdict |
| The negative total exceeds the positive total | Sentences like `계약번호 확인 후 연락처 010-1234-5678`, where both kinds share a window, are common |

Vetoed candidates are recorded in `rejections` with the `BusinessContext` reason.

**This rule puts recall ahead of precision.** Because a passing check digit beats negative context, a random
ten-digit number that happens to satisfy the business registration check gets flagged even while labeled
`계약번호`. Between missing PII and masking one extra business number, the latter was chosen.

---

## 10. Masking

Policies apply to the **source spans** produced by detection. Detection runs on normalized text and masking
runs on the original, and since `SpanMap` is the only thing joining them, the responsibility for recovery
correctness lives in one place.

### Two Guarantees

| Guarantee | Scope |
| --- | --- |
| Bytes **outside** detected spans are identical to the source | Every policy |
| `unmask(mask(text).text, &map)` equals the source exactly | Tokenize policy |

The first matters more than the second. Once it holds, masking cannot quietly corrupt a document. The
implementation mirrors that shape directly: output is a loop of "copy verbatim from the end of the previous
span to the start of this one, then substitute the span", with no path that parses or reassembles the source.

### Four Policies

| Policy | Example output | Reversible |
| --- | --- | --- |
| `Redact(Label)` | `[주민등록번호]` | No. The kind remains, in **Korean** |
| `Redact(Code)` | `[RESIDENT]` | No. The kind remains, in **English** |
| `Redact(Fill('*'))` | `**************` | No. The length remains |
| `Redact(Fixed(s))` | a fixed string | No. Nothing remains |
| `Partial` | `010******5678` | No. Only the edges remain |
| `Hash` | `[PHONE:098844363e2d]` | No, but **equal values yield equal tokens**, so linkage analysis works |
| `Tokenize` | `[[PII:0]]` | **Fully reversible.** A restore map comes with it |

`Redact` takes four placeholder shapes, which is why it spans four rows, but it is one policy. `Hash`
requires the `hash` feature flag.

### Choosing the Placeholder Language

This library is Korean-specific, but **its users are not necessarily Korean speakers.** Engineers at
international companies handling Korean documents are a real part of the audience, and `[카드번호]` embedded
in their English report corrupts the document. That is what `Redact(Code)` is for.

| Name | Value | Where it is used |
| --- | --- | --- |
| `entity.label()` | `카드번호` | `Redact(Label)` placeholder |
| `entity.code()` | `credit_card` | JSON serialization, Python attributes, command-line output |
| `entity.code_upper()` | `CREDIT_CARD` | `Redact(Code)` placeholder, hash token prefix |

All three names are defined in one place, and a unit test checks that they never drift apart.

Hash token prefixes are always English regardless of policy, because they are values a machine matches on,
not placeholders a human reads.

```rust
use rust_pii_transformer::detect::{Config, EntityKind};
use rust_pii_transformer::mask::{mask, unmask, Policy, PolicySet, Redaction};

// Policies can differ per entity.
let policies = PolicySet::new(Policy::Redact(Redaction::Label))
    .with(EntityKind::Phone, Policy::Partial { keep_prefix: 3, keep_suffix: 4, fill: '*' });

let out = mask("카드 4111-1111-1111-1111 연락처 010-1234-5678", &Config::default(), &policies).unwrap();
assert_eq!(out.text, "카드 [카드번호] 연락처 010******5678");

// Use tokenize when the masking has to be reversible.
let text = "주민등록번호 팔팔공일공일 - 1234567 입니다";
let out = mask(text, &Config::default(), &PolicySet::new(Policy::Tokenize)).unwrap();
assert_eq!(unmask(&out.text, out.restore.as_ref().unwrap()).unwrap(), text);
```

### The Restore Map Is Itself PII

`RestoreMap` holds tokens paired with the original fragments. Storing it alongside the masked text defeats
the point of masking. The token prefix is chosen **after confirming the string does not occur in the
source**, so an original that already contains something like `[[PII:0]]` will not collide.

### Overlapping Detections

A finding that overlaps a preceding one is not applied; it is recorded in `MaskOutput::skipped`. Empty is
the normal case, and a non-empty list means the detection layer emitted overlapping results. Nothing is
swallowed silently.

---

## 11. Synthetic Validation Corpus

Real resident registration numbers cannot go into a test suite, so a generator for synthetic data with valid
check digits ships alongside. It is not a by-product but a module with independent value, and it is what
makes reporting recall and precision as numbers possible.

### The Generator Does Not Reimplement the Checks

Check digits are not recomputed from a second copy of the formula. The generator **runs the verifier from
`detect::checksum` directly**, trying 0 through 9 in the final position until one passes. Generator and
verifier cannot drift apart by construction. Writing the formula twice means that fixing one copy leaves the
corpus quietly lying.

### What It Produces

- Values for all ten entities. Those with check digits are generated to pass
- Nine notation variants of the same value: plain digits, hyphenated, spaced, full-width, Korean numerals,
  lookalike letters, partial masking, verb endings, and the canonical form
- Sentences with and without context cues
- Ten kinds of negative samples designed to induce false positives: order numbers, invoice numbers, tracking
  numbers, product codes, amounts, membership numbers, policy numbers, service phone numbers, contract
  numbers, and ordinary phrases built from numeral syllables (`이사 갑니다`, `사구 팔구`)

**The negative samples were not chosen to be easy to pass.** If a thirteen-digit tracking number happens to
satisfy Luhn, that is a genuine false positive, and that probability belongs in the numbers. All 16 remaining
false positives are exactly this: thirteen-digit tracking numbers passing Luhn, and ten-digit contract
numbers passing the business registration check.

```rust
use rust_pii_transformer::synth::corpus;

let samples = corpus(20260812, 40);
assert_eq!(samples, corpus(20260812, 40)); // same seed, same corpus, always
```

### Context-Required Entities Get No Context-Free Positive Samples

Bank account numbers, driver's license numbers, dates of birth, and passport numbers are designed not to be
reported without context. Samples like that have an ambiguous ground truth: the text does contain PII, but
the library deliberately does not report it, and that decision is the price of false positive suppression
rather than a recall loss. So they are excluded from the corpus, and the behavior itself is pinned by
separate unit tests.

**The same rule applies to notation variants.** Partial masking makes the check digit unusable, and verb
endings incur the cost of expanded syllables. Both fall below threshold without context by design, so
neither variant produces context-free positive samples.

---

## 12. Command-Line Tool rpit

Built with the `cli` feature flag. With no input it reads stdin, and with no `--output` it writes stdout, so
it drops straight into a pipe.

```bash
cargo build --features cli --release

# detect
rpit detect --text "주민등록번호 팔팔공일공일 - 1234567"
rpit detect --file report.txt --format json

# mask. tokenize requires a restore map path
rpit mask --file report.txt --policy tokenize --restore-map map.json --output masked.txt
rpit mask --text "연락처 010-1234-5678" --policy partial --keep-prefix 3 --keep-suffix 4

# choose the placeholder language. label is Korean, code is English
rpit mask --text "카드 4111-1111-1111-1111" --policy label   # 카드 [카드번호]
rpit mask --text "카드 4111-1111-1111-1111" --policy code    # 카드 [CREDIT_CARD]

# restore
rpit unmask --file masked.txt --restore-map map.json

# see why something was not flagged
rpit explain --text "접수번호 1234567890123 입니다"

# generate synthetic samples
rpit synth --rounds 1 --seed 3 --format json
```

`explain` reports accepted results alongside rejected candidates and their reasons. The human-readable
column is the Korean entity label, matching `Redact(Label)`. Use `--format json` for output keyed by the
English codes (`credit_card`, `bank_account`, and so on).

```text
탐지 0 건

떨어진 후보 2 건
  카드번호           1234567890123                사유 ChecksumFailed  점수 0.15
  계좌번호           1234567890123                사유 NoContext       점수 0.10
```

Running the tokenize policy without `--restore-map` is **refused.** A masked result produced without a map
cannot be reversed, and choosing a reversible policy means intending to reverse it.

---

## 13. Performance

Release build, averaged over 20 runs.

| Input character | Size | Total | Throughput | Normalization | Detection |
| --- | --- | --- | --- | --- | --- |
| Document with mixed PII | 86.6 KB | 3.23 ms | 25.6 MB/s | 1.15 ms | 2.08 ms |
| Ordinary prose | 65.0 KB | 0.46 ms | 135.9 MB/s | 0.46 ms | 0.00 ms |

For a single document of 866 bytes, latency over 2,000 measurements is a median of **0.019 ms**, with a 95th
percentile of 0.033 ms and a 99th percentile of 0.057 ms.

Text without PII runs more than five times faster because each pass falls through to an identity mapping when
it has nothing to convert, and because no candidate evaluation happens at all when there are no digit runs.

**These are throughput numbers, not accuracy numbers.** Recall and precision are reported in
[Measured Accuracy](#measured-accuracy) at the top of this document.

---

## 14. Python Bindings

PyO3 produces an abi3 (stable ABI) wheel, so it installs on Python 3.9 and later across platforms without a
Rust toolchain. All four layers are exposed.

### Installation

```bash
# After PyPI publication: no Rust toolchain needed, grab the abi3 wheel
pip install rust_pii_transformer

# From source (latest main / before publication): requires a Rust toolchain and maturin on the install machine
pip install maturin
maturin develop
```

`maturin develop` needs no `--features` argument. `pyproject.toml` declares
`features = ["python", "hash"]`, so hash pseudonymization comes along. A wheel is a distribution artifact
whose recipient cannot re-pick features, so all four policies have to be in it.

`maturin` requires a virtual environment. Create one first if there is none.

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate
# Linux, macOS: source .venv/bin/activate
```

What surface a given build exposes can be checked at runtime.

```python
import rust_pii_transformer as rpit

rpit.__version__            # '0.1.0'
rpit.__status__             # 'span, normalize, detect, and mask layers are available'
rpit.__has_hash_policy__    # True if this wheel was built with the hash feature
```

### All Four Layers

```python
import rust_pii_transformer as rpit

text = "주민등록번호 팔팔공일공일 - 1234567 이고 연락처는 010-1234-5678 입니다"

# detection
report = rpit.detect(text)
for f in report.findings:
    print(f.entity, f.certainty, f.score, f.text(text))
# resident probable 0.86 팔팔공일공일 - 1234567
# phone    probable 0.98 010-1234-5678

# why something was not flagged. 접수번호 is in the negative context dictionary,
# so both candidates are vetoed
for r in rpit.detect("접수번호 1234567890123").rejections:
    print(r.entity, r.reason)      # credit_card  business_context
                                   # bank_account business_context

# without negative context the reasons differ
for r in rpit.detect("첨부 자료 1234567890123 참고하세요").rejections:
    print(r.entity, r.reason)      # credit_card  checksum_failed
                                   # bank_account no_context

# a different policy per entity
policies = (rpit.PolicySet(rpit.Policy.redact_label())
            .with_entity("phone", rpit.Policy.partial(3, 4)))
rpit.mask(text, policies).text

# tokenize guarantees recovery. the map can be stored and applied in another process
out = rpit.mask(text, rpit.PolicySet(rpit.Policy.tokenize()))
blob = out.restore.to_json()
assert rpit.unmask(out.text, rpit.RestoreMap.from_json(blob)) == text
```

### Using Offset Mapping Directly

For writing your own normalization pass and mapping its output back to source coordinates.

```python
b = rpit.SpanMapBuilder()
b.keep("880101")
b.absorb("-", "separator.hyphen", "separator")
b.keep("1234567")
normalized, smap = b.finish()          # '8801011234567'
smap.validate()                        # raises SpanMapError if an invariant broke

src = smap.to_source(rpit.Span(0, 13, 0, 13))
src.byte_start, src.byte_end           # (0, 14) the hyphen is inside the source range
src.rules                              # ['separator.hyphen'] the evidence
```

### Character Offsets Are First-Class

Python strings are indexed by character, so handing back only byte offsets forces a conversion on the
consumer side, and that conversion becomes a fresh source of offset bugs. Every span therefore carries both
coordinates, and there is a dedicated entry point for the common case of knowing only character offsets.

```python
start = normalized.index("880101")     # Python's index is character-based
src = smap.to_source_from_chars(normalized, start, start + 6)
src.span.slice(source_text)            # slices the exact source fragment
```

### Public Surface

| Name | Contents |
| --- | --- |
| `detect(text, config=None)` | `Report` |
| `normalize(text, config=None)` | `Normalized` |
| `mask(text, policies=None, config=None)` | `MaskOutput` |
| `unmask(masked, restore_map)` | the recovered string |
| `entity_names()` | every entity name this build handles |
| `Config` | `min_score`, `min_context`, `min_veto`, `context_window`, `nfc`, `fold`, `hangul`, `lookalike`, `separator`, `numeral_*`, `weights`, `set_weights(...)` |
| `Report` | `findings`, `rejections`, `normalized_text`, `to_json` |
| `Finding` | `entity`, `entity_label`, `source`, `normalized`, `certainty`, `score`, `evidence`, `text(src)`, `to_json` |
| `Evidence` | `rule`, `checksum`, `checksum_reason`, `context_hits`, `normalizations`, `cost`, `snapped` |
| `Rejection` | `entity`, `source`, `reason`, `score` |
| `Policy` | `redact_label` (Korean), `redact_code` (English), `redact_fill`, `redact_fixed`, `partial`, `hash`, `tokenize` |
| `PolicySet(default=None)` | `with_entity`, `policy_for` |
| `MaskOutput` | `text`, `applied`, `skipped`, `restore` |
| `RestoreMap` | `prefix`, `entries`, `to_json`, `RestoreMap.from_json` |
| `Span(byte_start, byte_end, char_start, char_end)` | `Span.from_char_range(text, s, e)`, `slice(text)` |
| `SpanMapBuilder(text=None)` | `keep`, `replace`, `numeral`, `absorb`, `finish()` |
| `SpanMap` | `identity`, `compose`, `to_source`, `to_source_from_chars`, `validate`, `segments`, `to_json` |
| `SourceSpan`, `Segment`, `NormalizationCost` | span recovery results and introspection |
| `SpanMapError` | invariant violation, coordinate mismatch, bad restore token |

The module also carries `__version__`, `__status__`, and `__has_hash_policy__`. The last one reports at
runtime whether this wheel includes the hash pseudonymization policy.

### Enums Come Across as Strings

`entity`, `certainty`, `reason`, and `checksum` are all lowercase snake_case strings. They compare and
serialize directly in Python and work as dictionary keys as-is. The human-readable Korean name is
`entity_label`, and the names emitted by `to_json()` match the attribute values.

The third argument to `absorb` is one of `"whitespace"`, `"separator"`, or `"other"`. Each carries a
different penalty coefficient, so it has to be chosen correctly; anything else raises `ValueError`.

### Two Rules at the Boundary

- **Panics do not cross into Python.** The core's `Span::slice` panics out of range and `finish` consumes the
  builder. The binding checks first and converts those into `ValueError` and `RuntimeError`
- **The core stays clean.** No `#[pyclass]` is attached to `span` types; the attributes live only on wrappers
  inside `src/python.rs`. That is why PyO3 never enters the default build

### Keep Rule Names Constant

Rule names passed to `SpanMapBuilder` are interned internally and never reclaimed, because the core carries
rule names as `&'static str`. A fixed handful of names keeps the pool at a constant size, but generating a
new name every iteration, as in `f"rule.{i}"`, accumulates one entry per call. Pass variable values as other
arguments, not as rule names.

---

## 15. Known Limitations

### Out of Scope

| Item | Why |
| --- | --- |
| Names, addresses | Not decidable by rule. That is named entity recognition territory, and it does not belong in the core |
| Non-Korean identifiers | This library is for Korean text only. See the opening section |

### Where Verdicts Get Shaky

**A digit run without context is undecidable in principle.** Whether `1234567890` is an account number or an
order number cannot be known by rule. That is exactly why account numbers, driver's license numbers, and
dates of birth require context.

**Credit card and bank account numbers overlap at thirteen digits.** Check digits weigh more than context, so
a number inside account context that passes Luhn is reported as a credit card. That is where the 95 percent
recall and precision for bank account numbers in the corpus comes from. Raising the context weight fixes this
case and adds false positives elsewhere. The current balance is the default and is adjustable through
`Config`.

**Accidental check-digit passes cannot be prevented.** About 10 percent of random thirteen-digit numbers pass
Luhn, and about 10 percent of random ten-digit numbers pass the business registration check. Such tracking
numbers get flagged as credit cards and such contract numbers as business registration numbers. The negative
context dictionary aims at exactly this spot, but since a passing check digit is designed to beat negative
context, it does not fire here. That was judged better than missing PII.

### Scope of the Accuracy Numbers

The recall and precision in this document were measured on the
[synthetic validation corpus](#11-synthetic-validation-corpus). That is enough to prevent regressions and to
see the effect of a change, but it is not a measurement on real Korean documents. Measuring again on your own
data and tuning the thresholds is recommended.

---

## 16. Build and Test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings

# including opt-in features
cargo test  --features "cli,hash"
cargo clippy --features "cli,hash,python" --all-targets -- -D warnings

# runnable example
cargo run --example demo

# see accuracy and masking round-trips
cargo test --test accuracy -- --nocapture
```

### Python Extension Module

```bash
pip install maturin
maturin develop                       # pyproject.toml declares features = ["python", "hash"]
pytest tests/test_python_binding.py
```

To check without maturin, build the cdylib with cargo and rename it onto the import path. Turn on `hash`
alongside `python` to get the same surface the maturin wheel has; leaving it out produces a module without
the hash policy, where `__has_hash_policy__` is false.

```bash
cargo build --features "python,hash" --release
# Windows:  target/release/rust_pii_transformer.dll      -> rust_pii_transformer.pyd
# Linux:    target/release/librust_pii_transformer.so    -> rust_pii_transformer.so
# macOS:    target/release/librust_pii_transformer.dylib -> rust_pii_transformer.so
```

### Directory Layout

- `src/`
  - `lib.rs` public API entry point
  - `error.rs` the single error enum
  - `span.rs` offset mapping
  - `normalize/` `mod.rs`, `nfc.rs`, `fold.rs`, `hangul.rs`, `lookalike.rs`, `separator.rs`
  - `detect/` `mod.rs`, `checksum.rs`, `scanner.rs`, `context.rs`, `entity.rs`
  - `mask/` `mod.rs`, `policy.rs`, `restore.rs`
  - `synth/mod.rs` synthetic validation corpus generator
  - `bin/rpit.rs` command-line tool (`--features cli`)
  - `python.rs` PyO3 bindings (`--features python`)
- `examples/demo.rs` a live detection example
- `tests/accuracy.rs` recall, precision, and masking round-trip measurement
- `tests/test_python_binding.py` binding regression tests

---

## 17. License

Apache License 2.0
