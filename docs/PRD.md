# PRD: `codec` — A Memory-Safe H.264 Codec in Rust

**Status:** Draft v0.2
**Owner:** brandon.vrooman@innobit.io
**Target context:** user-generated content — video uploaded to a website, then streamed
**Last updated:** 2026-08-30

---

## 1. Summary

`codec` is a from-scratch implementation of the ITU-T H.264 | ISO/IEC 14496-10 (MPEG-4 AVC)
video codec written in Rust. It ships as an embeddable library plus a CLI, and is built to be a
**memory-safe alternative to `libavcodec`/`openh264` for applications that decode untrusted
H.264**.

The deployment context is a **UGC upload/streaming pipeline**: arbitrary video arrives from the
public internet, gets validated and processed server-side, and is streamed back out. That context
is what makes the memory-safety thesis load-bearing rather than academic — every byte the decoder
touches is attacker-supplied — and it sets the feature priorities in §3 and §4.2.

The product is delivered in two large arcs:

1. **Decoder** (this PRD's primary scope) — phased across Constrained Baseline → Main → High.
2. **Encoder** (scoped later, sketched in §11) — reuses the decoder's bitstream, transform,
   prediction, and reconstruction primitives.

Nothing in the core codec wraps an existing C implementation. FFmpeg, the JM reference decoder,
and `openh264` appear only as **test oracles**, never as dependencies.

---

## 2. Problem & Motivation

H.264 remains the most widely deployed video codec in the world, and essentially every production
decoder is written in C or C++. Video decoders are among the most attacker-exposed parsers in a
modern application: they consume fully attacker-controlled, deeply structured binary input, and
they have a long history of exploitable memory-safety bugs. A decoder that is memory-safe *by
construction* removes an entire bug class from that attack surface.

Today a Rust application that needs to decode H.264 has three unappealing options: link
`libavcodec` (C, large trusted computing base, awkward cross-compilation), link `openh264` (C++,
Constrained Baseline only in practice), or shell out to `ffmpeg`. There is no mature, pure-Rust,
conformant H.264 decoder.

### Why "from scratch"

Wrapping a C codec preserves the exact property we are trying to eliminate. A safe wrapper around
an unsafe parser is still an unsafe parser. The value of this project is in the implementation
itself, not in the API surface.

---

## 3. Goals & Non-Goals

### 3.1 Goals

| # | Goal |
|---|------|
| G1 | **Conformance.** Bit-exact output on the ITU-T H.264.1 conformance bitstream suite for every profile/feature we claim to support. |
| G2 | **Memory safety.** No `unsafe` in the core decode path. No panics, aborts, OOMs, or unbounded loops on *any* input, valid or malformed. |
| G3 | **Robustness.** Malformed, truncated, and fuzzed bitstreams produce a typed error or a best-effort concealed frame — never undefined behavior and never a crash. |
| G4 | **Real-world usability.** Handles the files people actually have: Annex B streams and MP4/ISOBMFF (AVCC) files, not just hand-fed NAL units. |
| G5 | **Competitive-enough performance.** Real-time 1080p decode on a single modern core; see §7 for the numeric bar. |
| G6 | **Embeddability.** A small, stable, well-documented Rust API with predictable allocation behavior. |

**Priority order when goals conflict: G1 correctness → G2/G3 safety → G4/G6 usability → G5 speed.**
This ordering is normative, not a sentiment. Concretely: a performance target is never met by
weakening a correctness or safety guarantee, performance work does not begin until the relevant
conformance gate is green, and no performance number blocks a release (§7.3). If a reviewer has to
choose between a slower decoder and a less certain one, they pick the slower one.

### 3.2 Non-Goals (v1.0)

| Non-goal | Rationale |
|---|---|
| Interlaced coding (PicAFF / MBAFF, field pictures) | Roughly doubles the complexity of neighbor derivation, deblocking, MV prediction, and DPB management. Confirmed as excluded: the target is UGC upload/streaming, which is overwhelmingly progressive (§4.2). The residual interlaced tail is handled by the fallback path in N1.3, not by us. Deferred to v1.1; see R3. |
| 4:2:2, 4:4:4, monochrome-only, and >8-bit profiles (High 10 / 4:2:2 / 4:4:4 Predictive) | Professional/mezzanine formats outside the initial target. Chroma format and bit depth are parameterized in the design so this is additive, not a rewrite. |
| Extended profile (88), data partitioning (NAL types 2–4) | Effectively unused in the wild. |
| FMO (slice groups) and ASO (arbitrary slice order) | Baseline-only features excluded from Constrained Baseline; not present in real-world streams. |
| SVC (Annex G) and MVC (Annex H) | Separate codecs in practice. |
| Hardware acceleration (VA-API, VideoToolbox, D3D11VA, NVDEC) | Out of scope; a hardware path would bypass the memory-safety thesis entirely. |
| C ABI / FFI bindings | Explicitly descoped for v1.0. Revisit once the Rust API is stable. |
| Bit-exact match with x264's *encoder* decisions | Not a meaningful target; encoder quality is measured by BD-rate, not bit-exactness. |

### 3.3 Explicit anti-goals

- We will not add `unsafe` to win a benchmark. SIMD is opt-in, isolated, and separately audited
  (§6.5).
- We will not skip a conformance vector to hit a milestone date. A profile is "supported" only
  when its full vector set passes.

---

## 4. Users & Use Cases

| User | Use case | What they need from us |
|---|---|---|
| **Rust application developers** | Decoding user-uploaded or network video inside a service | Safe API, no C toolchain, typed errors, bounded memory |
| **Security-sensitive platforms** | Media processing on untrusted input (chat apps, UGC pipelines, sandboxed viewers) | The no-panic/no-`unsafe` guarantee, fuzzing evidence, an auditable codebase |
| **Embedded / WASM developers** | Playback where shipping `libavcodec` is impractical | `alloc`-only core, small binary, no build-time C dependency |
| **Tooling authors** | Bitstream inspection, conformance testing, research | Access to parsed syntax elements (SPS/PPS/slice headers/MB modes), not just pixels |
| **Codec engineers (us)** | Building the encoder in phase 5 | Shared primitives and a trusted decoder to verify against |

### 4.1 Primary user journeys

1. *"Decode this MP4 to frames."* — `codec decode input.mp4 -o out.y4m`, or three lines of library code.
2. *"Is this stream valid, and what's in it?"* — `codec inspect input.h264` prints NAL structure, SPS/PPS, per-slice headers.
3. *"Decode this hostile blob without getting owned."* — the library returns `Err(...)` and stays within its configured memory budget.

### 4.2 What the target input mix actually looks like

UGC uploads are not a uniform population, and the distribution drives the phasing:

| Source | Typical coding | Implication |
|---|---|---|
| Phone captures (iOS/Android) | High profile, 4:2:0 8-bit, progressive, MP4/`avcC` | The bulk of the volume. Needs **P3 complete** — this is why v1.0 is High, not Baseline. |
| `x264`/`ffmpeg` re-encodes, editing-app exports | High profile, CABAC, B-frames, occasionally 8×8 transform + custom scaling lists | Exercises the full Main + High feature surface, including weighted prediction and long-term refs. |
| Screen and game recorders (OBS, browser capture) | Constrained Baseline or Main, long GOPs, sometimes very high resolutions | Stresses DPB sizing and the resource limits in F4.4 more than it stresses coding tools. |
| WebRTC / conferencing captures | Constrained Baseline, CAVLC, no B-frames, frequent IDRs | Covered by **P1**; also the class most likely to be truncated or mid-GOP. |
| Legacy files (camcorder rips, TV captures, old transcodes) | Occasionally interlaced/MBAFF, occasionally Baseline with FMO | The long tail. Out of scope by §3.2 — must fail *precisely*, never silently. |
| Hostile uploads | Deliberately malformed | The entire premise of G2/G3. |

Two consequences are load-bearing:

- **Constrained Baseline alone is not a shippable product for this context**, so P1 is a stepping
  stone rather than a release. The first version worth deploying is **P3**.
- **The precision of `Error::Unsupported` (N1.3) is a product feature, not a nicety.** An upload
  pipeline that knows exactly which file it cannot handle can route that file to `ffmpeg` and keep
  the safe decoder on the other 99%. A decoder that fails vaguely forces the operator to route
  *everything* to `ffmpeg`, which throws away the reason to adopt us at all.

---

## 5. Functional Requirements

Requirements are grouped by the phase that delivers them. Each phase is independently shippable
and is gated on its conformance criteria (§8).

### 5.1 Phase 0 — Foundations

| ID | Requirement |
|---|---|
| F0.1 | Bit reader with Exp-Golomb decoding: `u(n)`, `f(n)`, `ue(v)`, `se(v)`, `me(v)`, `te(v)`; `more_rbsp_data()`; every read returns `Result` on exhaustion. |
| F0.2 | Annex B byte-stream parsing: start-code scanning (3- and 4-byte), NAL unit delimitation, emulation-prevention byte (`0x03`) removal to produce RBSP. |
| F0.3 | AVCC/`avcC` length-prefixed NAL parsing (1/2/4-byte lengths) with SPS/PPS extraction from the decoder configuration record. |
| F0.4 | NAL unit header parsing: `nal_ref_idc`, `nal_unit_type`; dispatch for types 1, 5, 6, 7, 8, 9, 12; graceful skip of unknown/reserved types. |
| F0.5 | SPS parsing: profile/level/constraint flags, `chroma_format_idc`, bit depths, `log2_max_frame_num`, POC type 0/1/2 parameters, `max_num_ref_frames`, frame dimensions and cropping, `frame_mbs_only_flag`, VUI (including `bitstream_restriction` → `max_num_reorder_frames`, `max_dec_frame_buffering`), and High-profile scaling list syntax. |
| F0.6 | PPS parsing: entropy mode, `num_slice_groups`, ref index defaults, weighted-prediction flags, QP/chroma offsets, deblocking control flag, `constrained_intra_pred_flag`, `transform_8x8_mode_flag`, picture scaling lists. |
| F0.7 | SEI parsing sufficient to reach `recovery_point` (random access without IDR); unknown SEI payloads skipped safely. |
| F0.8 | Level-table lookup (`MaxDpbMbs`, `MaxFS`, `MaxMBPS`) used to size and bound the DPB. |

### 5.2 Phase 1 — Constrained Baseline decoder

Target: profile_idc 66 with `constraint_set1_flag`, 4:2:0 8-bit progressive, CAVLC, I/P slices.

| ID | Requirement |
|---|---|
| F1.1 | Slice header parsing for I and P slices, including ref list reordering and `dec_ref_pic_marking`. |
| F1.2 | CAVLC residual decoding: `coeff_token` with `nC` derived from neighboring blocks, trailing-ones signs, `level_prefix`/`level_suffix` with suffix-length adaptation, `total_zeros`, `run_before`. |
| F1.3 | Macroblock layer for I and P: `mb_type` tables, `coded_block_pattern` (`me(v)` mapping), `mb_qp_delta`, sub-macroblock partitioning to 4×4, `I_PCM`. |
| F1.4 | Inverse scan (zigzag), dequantization (`LevelScale`, QP derivation, chroma QP mapping with `chroma_qp_index_offset`), 4×4 integer inverse transform, 4×4 luma DC Hadamard for `Intra_16x16`, 2×2 chroma DC Hadamard. |
| F1.5 | Intra prediction: `Intra_4x4` (9 modes with predicted-mode derivation), `Intra_16x16` (4 modes), chroma 8×8 (4 modes), availability rules, `constrained_intra_pred_flag`. |
| F1.6 | Inter prediction (P): partitions 16×16/16×8/8×16 and sub-partitions to 4×4, median MV prediction with the 16×8/8×16 directional exceptions, `P_Skip` MV derivation, luma 6-tap half-pel interpolation (`[1,−5,20,20,−5,1]`/32) plus quarter-pel bilinear averaging, chroma 1/8-pel bilinear, edge-extended reference fetch. |
| F1.7 | In-loop deblocking filter: boundary-strength derivation, α/β threshold tables from `indexA`/`indexB`, luma and chroma filtering for bS 1–3 and bS 4, `disable_deblocking_filter_idc` 0/1/2, slice α/β offsets. |
| F1.8 | DPB: POC type 0/1/2 computation, short-term reference list initialization for P, sliding-window marking, IDR handling (`no_output_of_prior_pics_flag`, `long_term_reference_flag`), Annex C output bumping ordered by POC. |
| F1.9 | Multi-slice pictures; slice-boundary neighbor availability handled correctly. |
| F1.10 | Frame output: planar YUV 4:2:0 with cropping applied, plus POC, `frame_num`, and VUI-derived color metadata. |

### 5.3 Phase 2 — Main profile

| ID | Requirement |
|---|---|
| F2.1 | CABAC: arithmetic decoding engine (clause 9.3) — `DecodeDecision`, renormalization, bypass, terminate — plus context initialization from `cabac_init_idc` and slice QP across the full context table. |
| F2.2 | CABAC binarization and context derivation for every syntax element: `mb_type`, `sub_mb_type`, MVD, ref idx, CBP, `mb_qp_delta`, `coded_block_flag`, `significant_coeff_flag`/`last_significant_coeff_flag`, `coeff_abs_level_minus1`, `end_of_slice_flag`. |
| F2.3 | B slices: bi-prediction, `L0`/`L1` list construction and reordering, `B_Skip`/`B_Direct_16x16`/`B_Direct_8x8`, spatial and temporal direct modes (`direct_8x8_inference_flag`, co-located MB derivation). |
| F2.4 | Weighted prediction: explicit (P and B, `pred_weight_table`) and implicit (B, distance-derived weights). |
| F2.5 | Long-term references and adaptive marking via MMCO operations 1–6. |
| F2.6 | `gaps_in_frame_num_value_allowed_flag` handling — synthesis of "non-existing" frames. |
| F2.7 | Reference-list size up to 16 frames; correct behavior at DPB capacity for the stream's level. |

### 5.4 Phase 3 — High profile (progressive, 4:2:0 8-bit)

| ID | Requirement |
|---|---|
| F3.1 | 8×8 transform: `transform_size_8x8_flag`, 8×8 inverse transform, 8×8 scan, 8×8 dequant. |
| F3.2 | `Intra_8x8` prediction: 9 modes with the reference-sample smoothing filter. |
| F3.3 | Scaling lists: sequence- and picture-level, fallback rules A and B, `Flat_4x4_16`/`Flat_8x8_16` defaults. |
| F3.4 | CABAC and CAVLC residual paths extended to 8×8 blocks; deblocking respects 8×8 transform edges. |
| F3.5 | `qpprime_y_zero_transform_bypass_flag` (lossless) handling — or explicit, tested rejection with a typed error if descoped. |

### 5.5 Phase 4 — Hardening & performance

| ID | Requirement |
|---|---|
| F4.1 | Optional SIMD backends (x86-64 SSE2/AVX2, aarch64 NEON) for interpolation, transforms, deblocking, and pixel copy — behind the `simd` feature, runtime-dispatched, bit-exact against the scalar path. |
| F4.2 | Frame-level multithreaded decode with correct reference-availability synchronization; slice-level parallelism where the stream allows. |
| F4.3 | Error concealment policy: on a corrupt slice, choose between (a) typed error, (b) drop, (c) conceal from the previous frame — configurable, defaulting to typed error. |
| F4.4 | Bounded, configurable resource limits: max resolution, max DPB frames, max total allocation; exceeded limits are typed errors, never allocation failures. |

### 5.6 Bitstream & container I/O

| ID | Requirement |
|---|---|
| F5.1 | Annex B reader (streaming, incremental, tolerant of arbitrary chunk boundaries). |
| F5.2 | ISOBMFF/MP4 demuxer: `moov`/`trak`/`stsd`/`avc1`/`avcC` parsing, sample tables (`stts`, `stsc`, `stsz`, `stco`/`co64`, `ctts`, `stss`), fragmented MP4 (`moof`/`traf`/`trun`) support. |
| F5.3 | AVCC ↔ Annex B conversion in both directions. |
| F5.4 | Y4M and raw planar YUV writers for decoded output. |
| F5.5 | Container parsing is subject to the same no-panic/no-`unsafe` guarantees as the codec. |

### 5.7 CLI

Binary name: `codec`.

| ID | Requirement |
|---|---|
| F6.1 | `codec decode <in> -o <out.y4m\|out.yuv>` — Annex B or MP4 in, Y4M or raw planar out. |
| F6.2 | `codec inspect <in>` — NAL-by-NAL structure dump; `--sps`, `--pps`, `--slices`, `--mb` verbosity levels; machine-readable `--json`. |
| F6.3 | `codec compare <a.yuv> <b.yuv>` — bit-exactness check plus PSNR/SSIM for encoder work later. |
| F6.4 | Useful exit codes and diagnostics (byte offset + syntax element on parse failure). |
| F6.5 | `codec encode ...` reserved for phase 5. |

### 5.8 Library API

Design constraints: no global state; no I/O inside the codec crate; caller controls buffers and
threading; parsed syntax exposed for tooling.

```rust
// Sketch — not final.
let mut dec = Decoder::builder()
    .limits(Limits { max_width: 3840, max_height: 2160, max_dpb_frames: 16, ..default() })
    .error_policy(ErrorPolicy::Strict)
    .build()?;

// Push NAL units (or whole Annex B chunks); pull frames as they become outputtable.
dec.push(nal_unit)?;
while let Some(frame) = dec.next_frame()? {
    let (y, u, v) = (frame.plane(Plane::Y), frame.plane(Plane::U), frame.plane(Plane::V));
    // frame.width(), .height(), .stride(), .poc(), .color_info()
}
dec.flush()?; // drain the DPB at end of stream
```

| ID | Requirement |
|---|---|
| F7.1 | Push/pull (sans-I/O) decoder core so the same code works sync, async, and in WASM. |
| F7.2 | Errors are a non-exhaustive typed enum distinguishing *unsupported* (valid stream, feature we don't implement), *invalid* (spec violation), and *truncated*. Never a stringly-typed error. |
| F7.3 | Frames expose planes as borrowed slices with explicit strides; no forced copies. |
| F7.4 | Public API is `#![deny(missing_docs)]` with doc examples that run in CI. |
| F7.5 | Semver discipline from v1.0; `Limits`, `Config`, and error enums are `#[non_exhaustive]`. |

---

## 6. Architecture

### 6.1 Workspace layout

```
codec/
├── crates/
│   ├── h264/               # core codec: decoder now, encoder in phase 5
│   ├── h264-bitstream/     # bit reader, Exp-Golomb, Annex B, AVCC, NAL, RBSP
│   ├── h264-containers/    # ISOBMFF/MP4 demux, Y4M/YUV writers
│   └── codec-cli/          # `codec` binary
├── fuzz/                   # cargo-fuzz targets
├── xtask/                  # conformance runner, benchmark driver, vector fetch
├── tests/conformance/      # vector manifests + expected checksums (not the vectors)
└── docs/                   # this PRD, spec-mapping notes, ADRs
```

### 6.2 Decoder pipeline

```
bytes → [Annex B / AVCC split] → NAL → RBSP → { SPS | PPS | SEI | slice }
                                                            │
        slice header ──────────────────────────────────────┤
                                                            ▼
                                          ┌── entropy decode (CAVLC | CABAC)
                                          │        ↓
                                          │   MB syntax + residual coefficients
                                          │        ↓
                                          │   inverse scan → dequant → inverse transform
                                          │        ↓
                        prediction ───────┴→ intra (4×4/8×8/16×16/chroma)
                                             inter (MC from DPB refs, weighted pred)
                                                     ↓
                                             reconstruct (pred + residual, clipped)
                                                     ↓
                                             deblocking filter (in-loop)
                                                     ↓
                                          DPB: marking, storage, POC-ordered output
```

### 6.3 Module decomposition (`h264` crate)

| Module | Responsibility |
|---|---|
| `nal` | NAL header, dispatch |
| `sps`, `pps`, `sei` | Parameter set and SEI parsing, activation, and validation |
| `slice::header` | Slice header parsing, ref list init/reorder, marking commands |
| `entropy::cavlc`, `entropy::cabac` | Entropy layers behind a common residual/syntax trait |
| `macroblock` | MB/sub-MB types, neighbor derivation, CBP, QP |
| `transform` | Scan, dequant, 4×4/8×8 inverse transforms, Hadamard DC |
| `pred::intra`, `pred::inter` | Prediction, interpolation, MV prediction, direct modes, weighting |
| `deblock` | Boundary strength, α/β filtering |
| `dpb` | POC, reference marking (sliding window + MMCO), output bumping |
| `frame` | Picture buffers, planes, strides, pooling |
| `limits` | Level tables, configured resource caps |

### 6.4 Key design decisions

- **Sans-I/O core.** The decoder never reads from a file or socket. All I/O lives in
  `h264-containers` and the CLI. This is what makes WASM and async embedding free.
- **Parameter-set activation is explicit.** SPS/PPS are stored by ID and *activated* per the
  spec's activation rules; changing an active SPS mid-stream triggers a controlled reconfiguration
  rather than silent reuse of stale state.
- **Frame buffer pooling.** The DPB owns a pool sized from level limits; steady-state decoding
  performs zero allocations. This is both a performance property and a robustness property.
- **Neighbor derivation is centralized.** Availability (A/B/C/D neighbors, slice boundaries,
  `constrained_intra_pred`) is computed in one place and reused by intra prediction, MV
  prediction, CAVLC `nC`, CABAC contexts, and deblocking. Duplicating this logic is the single
  most reliable source of conformance bugs in H.264 implementations.
- **Chroma format and bit depth are parameters, not constants**, even though v1.0 only exercises
  4:2:0 8-bit. This keeps High 10/4:2:2 additive.

### 6.5 The `unsafe` policy

- Every crate carries `#![forbid(unsafe_code)]` by default.
- The `simd` feature enables exactly one module per architecture that downgrades to
  `#![deny(unsafe_op_in_unsafe_fn)]`. That module contains only intrinsics, is entered through a
  runtime feature-detection gate, and has no control flow that depends on bitstream contents
  beyond bounds already validated by the scalar caller.
- Every SIMD kernel has a differential test against its scalar twin over randomized and
  conformance-derived inputs. A SIMD kernel that is not bit-exact with scalar is a release blocker.
- The default build (no features) has **zero** `unsafe` in the entire dependency tree; this is
  enforced in CI (`cargo-geiger` or equivalent).

---

## 7. Non-Functional Requirements

### 7.1 Correctness

| ID | Requirement |
|---|---|
| N1.1 | 100% pass rate on the ITU-T H.264.1 conformance vectors applicable to each supported profile, verified by MD5 of the decoded output against the supplied reference YUV. |
| N1.2 | Bit-exact agreement with `ffmpeg`'s decoder across a corpus of ≥500 real-world streams (varied encoders, resolutions, GOP structures). |
| N1.3 | Any stream we cannot decode returns `Error::Unsupported` with the specific feature named — never wrong pixels. |

### 7.2 Robustness & security

| ID | Requirement |
|---|---|
| N2.1 | No panic, abort, or hang on any input. Enforced by continuous fuzzing; a panic found by fuzzing is a P0 bug. |
| N2.2 | ≥ 1000 CPU-hours of fuzzing without a new crash before each release, across ≥ 4 targets (NAL split, parameter sets, full decode, MP4 demux). |
| N2.3 | Peak memory bounded by `Limits`, independent of bitstream contents. A stream claiming 65535×65535 is rejected at SPS parse, not at allocation. |
| N2.4 | Decode time bounded: no input causes superlinear work relative to its byte length (guards against decompression-bomb-style slice/MB loops). |
| N2.5 | All arithmetic on bitstream-derived values uses explicit wrapping/saturating/checked semantics; release builds are as correct as debug builds. |

### 7.3 Performance

Per the priority order in §3.1, **performance is secondary to correctness and these numbers do not
gate a release.** They are directional targets: they tell us when to stop optimizing, they catch
regressions, and they are tracked publicly — but a v1.0 that is conformant, safe, and 20% slow
ships, while one that is fast and fails a conformance vector does not.

Measured on a modern x86-64 desktop core, 8-bit 4:2:0, single-threaded unless noted, decode-only
(no display, no color conversion), against `ffmpeg -threads 1` as the baseline.

| Content | v1.0 target (directional) | v1.1 target (`simd`) |
|---|---|---|
| 720p30 Main | ≥ 2× real-time | ≥ 5× real-time |
| 1080p30 Main | ≥ 1× real-time | ≥ 2.5× real-time |
| 1080p30 High | ≥ 1× real-time | ≥ 2× real-time |
| Ratio vs. `libavcodec` (1 thread) | ≤ 3.0× slower | ≤ 1.6× slower |
| Frame-parallel scaling (8 cores) | — | ≥ 4× over single-thread |

Steady-state decoding allocates zero bytes per frame — this one *is* a hard requirement, because it
is a robustness property (N2.3) that happens to also be a performance property.

Benchmarks run in CI on a fixed corpus with regression alerts at >5% change. A regression alert is a
tracked issue, not a merge blocker; a conformance or fuzzing regression is a merge blocker.

### 7.4 Portability

- Tier 1: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.
- Tier 2: `wasm32-unknown-unknown`, `aarch64-unknown-linux-gnu`.
- Core crates are `alloc`-only (no `std` requirement) so embedded and WASM targets work without
  feature gymnastics. `std` is a default-on feature.
- MSRV declared and tested in CI; bumps are minor-version events.

### 7.5 Code quality

- `clippy -D warnings`, `rustfmt`, `deny(missing_docs)` on public items.
- Every syntax-parsing function cites its spec clause in a doc comment
  (e.g. `/// ITU-T H.264 clause 7.3.2.1.1`). Spec traceability is a hard review requirement —
  it is what makes conformance bugs findable.
- Minimal dependencies in the core crates; each new dependency needs justification in review.

---

## 8. Testing & Verification Strategy

Correctness for a codec is not a matter of opinion, and the test strategy is the heart of this
project.

### 8.1 Conformance suite (primary gate)

The ITU-T H.264.1 conformance bitstreams are the authoritative oracle. Each vector ships with a
reference decoded YUV; we compare MD5 digests.

| Phase | Vector families (examples) | Exit criterion |
|---|---|---|
| 1 — Constrained Baseline | Baseline CAVLC I/P vectors (e.g. `BA*_*`, `CI_MW_D`, `NL*_Sony_*`, `SVA_*`, `CVPCMNL1_SVA_C` for I_PCM, `BAMQ*` for QP variation) | 100% of applicable Baseline vectors bit-exact |
| 2 — Main | CABAC families (`CABA*`), CAVLC-Main (`CANL*`), B-slice and weighted-prediction vectors, MMCO/long-term vectors | 100% of applicable progressive Main vectors bit-exact |
| 3 — High | FRExt suite (8×8 transform, scaling lists) | 100% of applicable progressive High 4:2:0 8-bit vectors bit-exact |

Interlaced/MBAFF vectors are tracked as **known-excluded** with an explicit list, so "pass rate"
is never inflated by silently skipping hard cases. `xtask conformance` fetches vectors (not
checked into git), runs the suite, and prints a pass/fail/excluded table.

### 8.2 Differential testing

`xtask difftest` decodes a large real-world corpus with both `codec` and `ffmpeg` and compares
per-frame MD5s. This catches the cases conformance vectors miss: encoder quirks, odd cropping,
unusual DPB churn, streams that start mid-GOP.

### 8.3 Fuzzing

| Target | Input |
|---|---|
| `fuzz_nal_split` | Raw bytes → Annex B/AVCC splitting |
| `fuzz_parameter_sets` | Raw bytes → SPS/PPS/SEI parsing |
| `fuzz_decode` | Raw bytes → full decode loop (seeded with conformance vectors) |
| `fuzz_decode_structured` | Mutation of *valid* streams (bit flips on real vectors) — finds deeper bugs than random bytes |
| `fuzz_mp4` | Raw bytes → ISOBMFF demux |

Corpus is persisted across CI runs. OSS-Fuzz integration is a v1.0 stretch goal.

### 8.4 Unit & property tests

- Every transform has a round-trip or reference-vector test.
- Interpolation, deblocking, and dequant are tested against independently derived reference
  implementations transcribed directly from the spec text.
- CABAC engine tested against captured decision traces.
- Property tests: bit reader never reads past the end; MV clamping stays in the extended plane;
  QP derivation stays in range for all inputs.

### 8.5 CI gates

Every PR: build (all tiers), `clippy -D warnings`, `fmt --check`, unit tests, the fast conformance
subset, `cargo-deny`, and a short fuzz smoke run. Nightly: full conformance suite, full differential
corpus, extended fuzzing, benchmark regression check.

---

## 9. Milestones

Sizing assumes one engineer working with AI assistance; ranges reflect the real variance in codec
work (conformance debugging dominates and is hard to schedule).

| Phase | Deliverable | Exit criteria | Est. |
|---|---|---|---|
| **P0** | Workspace, CI, bit reader, Annex B/AVCC, NAL, SPS/PPS/SEI parsing, `codec inspect`, fuzz targets, conformance harness | `inspect` prints correct structure for all conformance vectors; parser fuzzing clean | 2–3 wks |
| **P1a** | I-slice decode (intra, CAVLC, transform, deblocking, single frame) | All-intra vectors bit-exact | 3–5 wks |
| **P1b** | P-slice decode (inter prediction, MC, MV prediction, DPB, multi-slice) | Constrained Baseline vectors 100% bit-exact; `codec decode` produces correct Y4M | 4–6 wks |
| **P2a** | CABAC engine + all binarizations | Main-profile CABAC I/P vectors bit-exact | 3–5 wks |
| **P2b** | B-slices, direct modes, weighted prediction, MMCO/long-term | Progressive Main vectors 100% bit-exact | 4–6 wks |
| **P3** | High profile: 8×8 transform, `Intra_8x8`, scaling lists | Progressive High 4:2:0 8-bit vectors 100% bit-exact | 3–4 wks |
| **P4** | MP4 demux, hardening, resource limits, SIMD, multithreading, docs, API freeze → **v1.0** | 1000 fuzz-hours clean; differential corpus clean; resource limits enforced under fuzzing. Performance measured and published, not gated (§7.3) | 5–8 wks |
| **P5** | Encoder (separate PRD) | See §11 | TBD |

Deliberate sequencing note: **CABAC before B-slices.** Both are large, and debugging them
simultaneously means never knowing which one is wrong. CABAC lands first because it can be
validated on I/P streams where the rest of the pipeline is already known-good.

---

## 10. Success Metrics

| Metric | v1.0 target |
|---|---|
| Conformance pass rate (claimed profiles) | 100% |
| Real-world differential corpus agreement | ≥ 99.5% of streams bit-exact; 100% of failures triaged and documented |
| Fuzz hours without a crash | ≥ 1000 |
| `unsafe` blocks in default build | 0 |
| 1080p30 Main single-thread decode | ≥ real-time — *tracked, non-blocking* |
| Public API documented | 100% of public items, with runnable examples |
| Time from `cargo add` to a decoded frame | ≤ 10 lines of code |

---

## 11. Phase 5 — Encoder (forward-looking scope)

Sketched here so shared primitives are designed with it in mind; a full PRD follows once the
decoder ships.

- **Scope:** Constrained Baseline encoder first (I/P, CAVLC), then Main (CABAC, B-frames).
- **Shared with the decoder:** transforms, scan tables, quantization tables, intra/inter
  prediction, MC interpolation, deblocking, bitstream writing (mirror of the reader), DPB logic.
- **New:** rate-distortion optimization, mode decision, motion estimation, rate control (CQP, CRF,
  ABR, VBV), GOP structure, lookahead.
- **Verification:** every encoded stream must decode bit-exactly with both our decoder and
  `ffmpeg`; quality measured by BD-rate against `x264` at matched presets. Matching `x264`'s
  quality is explicitly *not* a v1 goal — being correct, conformant, and within a defined BD-rate
  band of `x264 --preset medium` is.
- **Risk:** encoder quality is an open-ended optimization problem. Scope it by preset tier, not by
  "beat x264."

---

## 12. Risks

| ID | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | **Conformance debugging is unpredictable.** A single wrong neighbor-availability rule can fail dozens of vectors with no obvious signal. | Schedule slip | Build the vector harness in P0, before any decoding. Add MB-level trace comparison against the JM/ffmpeg trace output early — finding *which macroblock* diverges is 90% of the debugging. |
| R2 | **CABAC is a large, unforgiving surface.** Context tables are long and transcription errors are silent. | Multi-week slip | Generate context tables from the spec tables programmatically; verify against captured decision traces; land CABAC on I/P only. |
| R3 | **Excluding interlace may block real users.** Broadcast/legacy content is often MBAFF. | Reduced addressable use cases | Report `Error::Unsupported` precisely so callers can fall back. Re-evaluate for v1.1 based on demand; the DPB and neighbor abstractions are designed to accept field pictures later. |
| R4 | **Safe-Rust bounds checking costs performance.** | Perf targets missed | Design for slice-based plane access with hoisted bounds checks; profile early, not at the end; `simd` feature as the escape valve. Largely de-risked by §3.1: performance targets are directional and do not gate a release, so the failure mode here is a slower v1.0, not a blocked one. |
| R5 | **Conformance vector access.** The suite is distributed by ITU-T and mirrors vary in completeness. | Weakened primary oracle | Confirm access in P0. If incomplete, weight differential testing against `ffmpeg` more heavily and document the gap honestly. |
| R6 | **H.264 patent licensing.** H.264 is covered by a patent pool (Via LA, formerly MPEG LA). Implementing and distributing a codec has licensing implications that are a *business* question, not a technical one, and they differ for decoders vs. encoders and for open-source vs. commercial distribution. | Distribution/legal | Flagged as an open question (§13, Q1) for the owner to resolve with counsel before public release. This is not legal advice and the engineering plan does not depend on the answer. |
| R7 | **Scope creep into "just add 10-bit / just add interlace."** | Never shipping v1.0 | Non-goals in §3.2 are contractual. New features go to v1.1 unless they block a conformance gate. |

---

## 13. Open Questions

| # | Question | Needed by |
|---|---|---|
| Q1 | Patent licensing posture for distribution (see R6) — and does it differ for the encoder? | Before public release |
| Q2 | Software license: MIT/Apache-2.0 dual (Rust ecosystem norm), or something more restrictive? | P0 |
| Q3 | MSRV policy: latest stable, or N-2 releases? | P0 |
| Q4 | Is WASM a first-class target (affects threading model and binary-size budget) or best-effort? | P0 |
| Q5 | Should `inspect` output stabilize as a supported machine-readable format (i.e. semver'd), or stay a debugging tool? | P4 |
| Q6 | Error-concealment default: strict errors, or best-effort playback? Player embedders and security embedders want opposite defaults. | P4 |
| Q7 | Does anyone need 10-bit / 4:2:2 badly enough to reorder it ahead of the encoder? | After v1.0 |

Q2 and Q3 are needed to scaffold the P0 workspace and are the next decisions required.

### 13.1 Decisions log

| Date | Decision | Consequence |
|---|---|---|
| 2026-08-30 | **Target is UGC upload/streaming**, not broadcast. | Confirms the interlace/MBAFF exclusion (§3.2); adds the input-mix analysis in §4.2; makes P3/High — not P1 — the first deployable version. |
| 2026-08-30 | **CABAC is sequenced before B-slices** (P2a before P2b). | Each large feature lands against a pipeline already proven by conformance vectors. |
| 2026-08-30 | **Correctness is primary; performance is secondary.** | Priority order added to §3.1; §7.3 targets demoted to directional; performance removed from the P4 release gate and marked non-blocking in §10. |

---

## 14. References

- ITU-T Rec. H.264 (MPEG-4 Part 10 / AVC) — the normative specification.
- ITU-T Rec. H.264.1 — conformance specification and bitstream suite.
- ITU-T Rec. H.273 — color primaries, transfer characteristics, matrix coefficients (VUI semantics).
- ISO/IEC 14496-12 / 14496-15 — ISOBMFF and AVC file format (`avcC`).
- Richardson, *The H.264 Advanced Video Compression Standard*, 2nd ed. — readable companion, not normative.
