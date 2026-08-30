//! Bit-level reading for H.264 bitstreams.
//!
//! This crate is the lowest layer of the [`codec`] H.264 implementation. It owns
//! everything between raw bytes and the syntax elements the codec consumes:
//!
//! - the fallible bit reader and Exp-Golomb decoding — ITU-T H.264 clause 9.1,
//! - Annex B byte-stream delimitation and emulation-prevention removal — clause 7.4.1.1,
//! - AVCC (`avcC`) length-prefixed NAL parsing — ISO/IEC 14496-15,
//! - NAL unit headers — clause 7.3.1.
//!
//! # Guarantees
//!
//! Every input to this crate is assumed to be attacker-controlled. Accordingly:
//!
//! - the crate contains no `unsafe` code, enforced by [`forbid(unsafe_code)`] at
//!   this crate root;
//! - no operation panics, however malformed the input — reads that run off the
//!   end of the data, or that would produce an out-of-range value, return an
//!   error instead;
//! - no operation allocates, and none loops for longer than the length of its
//!   input.
//!
//! [`codec`]: https://github.com/lushus-app/codec
//! [`forbid(unsafe_code)`]: https://doc.rust-lang.org/rustc/lints/levels.html#forbid
//!
//! # Status
//!
//! Scaffolding only. The bit reader lands next; see `docs/PRD.md` requirement F0.1.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
