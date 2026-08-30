# codec

A memory-safe H.264 (ITU-T H.264 | ISO/IEC 14496-10) codec written from scratch in Rust.

Status: **planning**. The product requirements document is the current source of truth:

- [`docs/PRD.md`](docs/PRD.md)

Scope in brief: a conformant H.264 **decoder** first — phased Constrained Baseline → Main →
High — built as an embeddable, `unsafe`-free library with a CLI, Annex B/MP4 bitstream I/O, and a
conformance + fuzzing test harness. An encoder follows in a later phase.
