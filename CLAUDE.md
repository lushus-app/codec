# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

`codec` is a from-scratch H.264 (ITU-T H.264 | ISO/IEC 14496-10) implementation in Rust,
targeting **UGC upload and streaming pipelines** — video arriving from the public internet,
processed server-side, streamed back out. It is positioned as a **memory-safe alternative to
`libavcodec`/`openh264`**, so every byte the decoder touches is assumed attacker-controlled.

**Current state: planning.** There is no code yet. `docs/PRD.md` is the source of truth for scope,
requirements, architecture, and phasing — read it before proposing or writing anything
substantial. Requirement IDs (`F1.6`, `N2.3`), risk IDs (`R4`), and question IDs (`Q1`) in that
document are stable; cite them in commits, PRs, and issues.

## Non-negotiable invariants

These are the product, not preferences. Violating one is a blocking review failure.

1. **Nothing wraps a C codec.** No `ffmpeg-sys`, no `openh264` bindings, no shelling out to
   `ffmpeg` from library code. Those tools are **test oracles only**, and they belong in
   `xtask/`, `tests/`, and `fuzz/` — never in `crates/`. A safe wrapper around an unsafe parser
   is still an unsafe parser; that is the whole reason this project exists.
2. **No `unsafe` in the default build.** Every crate carries `#![forbid(unsafe_code)]`. The sole
   exception is the per-architecture SIMD module behind the `simd` feature (PRD §6.5), which is
   intrinsics-only, runtime-gated, and must be bit-exact with its scalar twin. Do not add
   `unsafe` to win a benchmark.
3. **No panics on any input.** Indexing, slicing, arithmetic, and allocation on bitstream-derived
   values must be fallible or explicitly bounded. Prefer `get()` over `[]`, checked/saturating
   arithmetic over bare operators, and a typed error over `unwrap()`/`expect()`/`panic!`. A panic
   reachable from a bitstream is a P0 bug.
4. **Bounded memory.** Allocation sizes come from validated, clamped values — never directly from
   a header field. A stream claiming 65535×65535 is rejected at SPS parse, not at allocation.
5. **Correctness beats speed** (PRD §3.1). Performance targets are directional and do not gate a
   release. Do not start optimizing a component until its conformance gate is green.

## Scope boundaries

Out of scope for v1.0 — do not implement these, and do not "helpfully" add them (PRD §3.2):
interlaced coding (PicAFF/MBAFF, field pictures), >8-bit or 4:2:2/4:4:4 profiles, Extended
profile and data partitioning, FMO/ASO, SVC/MVC, hardware acceleration, C ABI/FFI bindings.

When the decoder meets one of these, it must return `Error::Unsupported` **naming the specific
feature**. That precision is a product requirement (PRD N1.3, §4.2), not a nicety: it lets an
upload pipeline fall back per-file instead of routing everything away from the safe decoder.

## Phasing

Work proceeds P0 → P1 → P2a → P2b → P3 → P4 (PRD §9). Each phase is gated on conformance vectors
and is independently shippable. Two ordering rules matter:

- **CABAC (P2a) lands before B-slices (P2b)**, so each large feature is debugged against a
  pipeline already proven by vectors.
- **P3/High is the first deployable version.** Constrained Baseline alone does not serve the
  target workload, so P1 is a stepping stone, not a release.

## Planned layout

```
crates/h264/              core codec (decoder now, encoder in phase 5)
crates/h264-bitstream/    bit reader, Exp-Golomb, Annex B, AVCC, NAL, RBSP
crates/h264-containers/   ISOBMFF/MP4 demux, Y4M/YUV writers
crates/codec-cli/         `codec` binary
fuzz/                     cargo-fuzz targets
xtask/                    conformance runner, benchmarks, vector fetch
tests/conformance/        vector manifests + expected checksums (vectors are NOT in git)
docs/                     PRD, spec-mapping notes, ADRs
```

The codec core is **sans-I/O**: it never opens a file or socket. All I/O lives in
`h264-containers` and the CLI. This is what keeps WASM and async embedding free.

## Conventions

- **Cite the spec.** Every function parsing or deriving a syntax element carries its clause in a
  doc comment: `/// ITU-T H.264 clause 7.3.2.1.1`. This is a hard review requirement — it is what
  makes conformance bugs findable months later.
- **Centralize neighbor derivation.** Availability (A/B/C/D neighbors, slice boundaries,
  `constrained_intra_pred`) is computed once and reused by intra prediction, MV prediction, CAVLC
  `nC`, CABAC contexts, and deblocking. Duplicating it is the single most reliable source of
  conformance bugs in H.264 implementations.
- **Error taxonomy.** Distinguish *unsupported* (valid stream, feature we don't implement),
  *invalid* (spec violation), and *truncated*. Never a stringly-typed error. Public error enums
  are `#[non_exhaustive]`.
- **Toolchain.** MSRV is **Rust 1.94** — a floor, not a moving target. License is
  `MIT OR Apache-2.0`; every manifest sets both `rust-version` and `license`.
- **Style.** `rustfmt` defaults, `clippy -D warnings`, `#![deny(missing_docs)]` on public items.

## Verifying work

Correctness here is objective — do not claim it without running the checks.

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo xtask conformance            # ITU-T H.264.1 vectors; fetches on first run
cargo xtask difftest               # per-frame MD5 vs. ffmpeg on a real-world corpus
cargo +nightly fuzz run fuzz_decode -- -max_total_time=60
```

Rules of engagement for these:

- **Never report a phase complete on unit tests alone.** A phase is done when its conformance
  vectors pass bit-exactly (MD5 against the reference YUV) — nothing less.
- **Never silently skip a failing vector.** Excluded vectors (interlaced/MBAFF) live in an
  explicit list so the pass rate is honest. Adding to that list needs a stated reason.
- **A fuzz-found panic is P0**, ahead of any feature work.
- When a vector fails, find *which macroblock* diverges before theorizing — MB-level trace
  comparison against ffmpeg/JM is the intended debugging path (PRD R1).

## Working in this repo

- Development happens on feature branches; PRs target `main`.
- The conformance vectors and the differential corpus are large and are **not** committed. Only
  manifests and expected checksums live in git.
- If you find a genuine problem with the PRD while implementing, say so and update `docs/PRD.md`
  in the same PR with a note in the decisions log (§13.1). Do not silently diverge from it.
