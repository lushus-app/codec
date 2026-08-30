//! Errors produced while reading an H.264 bitstream.

use core::fmt;

/// The result of a bitstream read.
pub type Result<T> = core::result::Result<T, BitstreamError>;

/// Something went wrong reading the bitstream.
///
/// The taxonomy deliberately separates *truncated* from *invalid* (PRD F7.2). A
/// truncated stream may simply not have arrived yet and a caller may choose to
/// wait for more bytes; an invalid one violates the specification and will not
/// become valid however much more data follows.
///
/// Every variant carries the bit offset at which the problem was detected, so a
/// caller can report a byte position and syntax element to a user (PRD F6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BitstreamError {
    /// The read ran past the end of the available data.
    Truncated {
        /// Bit offset from the start of the buffer at which the read began.
        at_bit: u64,
        /// How many bits the read needed.
        needed_bits: u32,
        /// How many bits remained.
        available_bits: u64,
    },
    /// The data is present but violates ITU-T H.264.
    Invalid {
        /// Bit offset from the start of the buffer at which the problem begins.
        at_bit: u64,
        /// What specifically is wrong.
        reason: InvalidReason,
    },
}

impl BitstreamError {
    /// The bit offset at which the problem was detected.
    #[must_use]
    pub fn at_bit(&self) -> u64 {
        match *self {
            Self::Truncated { at_bit, .. } | Self::Invalid { at_bit, .. } => at_bit,
        }
    }

    /// The byte offset at which the problem was detected.
    ///
    /// Note that this is an offset into the RBSP, which is the NAL unit payload
    /// *after* emulation prevention bytes have been removed. Mapping it back to
    /// a position in the original byte stream is the caller's job.
    #[must_use]
    pub fn at_byte(&self) -> u64 {
        self.at_bit() / 8
    }

    /// Whether more data could make this read succeed.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        matches!(*self, Self::Truncated { .. })
    }
}

/// Why a bitstream is invalid.
///
/// This is an enum rather than a string so that callers can match on it and so
/// that the set of things that can go wrong stays enumerable (CLAUDE.md, "Error
/// taxonomy").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidReason {
    /// An Exp-Golomb code began with 32 or more zero bits.
    ///
    /// ITU-T H.264 clause 9.1 bounds `ue(v)` to `[0, 2^32 - 2]`, which is
    /// reached with exactly 31 leading zeros. A longer run cannot encode a
    /// representable value, and accepting one would let a malformed stream
    /// drive an unbounded read (PRD N2.4).
    ExpGolombTooLong,
    /// `rbsp_stop_one_bit` was zero — ITU-T H.264 clause 7.3.2.11.
    MissingRbspStopBit,
    /// An `rbsp_alignment_zero_bit` was one — ITU-T H.264 clause 7.3.2.11.
    NonZeroRbspAlignmentBit,
    /// More than 32 bits were requested from a single read.
    ///
    /// This indicates a bug in the calling parser rather than bad data. It is
    /// reported as an error because no operation in this crate may panic.
    BitCountOutOfRange,
}

impl fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Truncated {
                at_bit,
                needed_bits,
                available_bits,
            } => write!(
                f,
                "truncated bitstream at bit {at_bit}: needed {needed_bits} bits, {available_bits} available"
            ),
            Self::Invalid { at_bit, reason } => {
                write!(f, "invalid bitstream at bit {at_bit}: {reason}")
            }
        }
    }
}

impl fmt::Display for InvalidReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match *self {
            Self::ExpGolombTooLong => "Exp-Golomb code has 32 or more leading zero bits",
            Self::MissingRbspStopBit => "rbsp_stop_one_bit is zero",
            Self::NonZeroRbspAlignmentBit => "rbsp_alignment_zero_bit is one",
            Self::BitCountOutOfRange => "more than 32 bits requested in a single read",
        };
        f.write_str(text)
    }
}

impl core::error::Error for BitstreamError {}
