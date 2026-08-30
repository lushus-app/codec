//! Tests for the bit reader.
//!
//! The Exp-Golomb cases are the code tables from ITU-T H.264 clause 9.1
//! transcribed directly, so a transcription error here shows up as a failing
//! test rather than as a conformance failure months later.

// Tests may index, panic, and do plain arithmetic; the library may not.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::format;
use std::vec::Vec;

use crate::{BitReader, BitstreamError, InvalidReason};

/// Packs a string of `0`/`1` into bytes, MSB first, zero-padding the last byte.
///
/// Underscores are ignored, so patterns may be grouped for readability.
fn bits(pattern: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut index = 0usize;
    for ch in pattern.chars() {
        if ch == '_' {
            continue;
        }
        assert!(
            ch == '0' || ch == '1',
            "pattern must be 0s, 1s and underscores"
        );
        if index.is_multiple_of(8) {
            out.push(0);
        }
        if ch == '1' {
            let last = out.len() - 1;
            out[last] |= 0x80 >> (index % 8);
        }
        index += 1;
    }
    out
}

#[test]
fn bits_helper_packs_msb_first_and_ignores_separators() {
    assert_eq!(bits("1"), [0b1000_0000]);
    assert_eq!(bits("0000_0001"), [0b0000_0001]);
    assert_eq!(bits("1000_0000_1"), [0b1000_0000, 0b1000_0000]);
    assert_eq!(bits("10110001_01"), bits("1011000101"));
    assert!(bits("").is_empty());
}

#[test]
fn reads_single_bits_across_byte_boundaries() {
    let data = bits("10110001_01");
    let mut reader = BitReader::new(&data);
    for expected in [
        true, false, true, true, false, false, false, true, false, true,
    ] {
        assert_eq!(reader.read_bit().unwrap(), expected);
    }
    assert_eq!(reader.bit_position(), 10);
}

#[test]
fn reads_fixed_width_values() {
    let data = [0xDE, 0xAD, 0xBE, 0xEF];
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_bits(0).unwrap(), 0);
    assert_eq!(reader.read_bits(4).unwrap(), 0xD);
    assert_eq!(reader.read_bits(12).unwrap(), 0xEAD);
    assert_eq!(reader.read_bits(16).unwrap(), 0xBEEF);
    assert_eq!(reader.bits_remaining(), 0);
}

#[test]
fn reads_a_full_32_bit_value() {
    let data = [0xFF, 0xFF, 0xFF, 0xFF];
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_bits(32).unwrap(), u32::MAX);
}

#[test]
fn rejects_reads_wider_than_32_bits() {
    let data = [0u8; 8];
    let mut reader = BitReader::new(&data);
    assert_eq!(
        reader.read_bits(33),
        Err(BitstreamError::Invalid {
            at_bit: 0,
            reason: InvalidReason::BitCountOutOfRange,
        })
    );
}

#[test]
fn reports_truncation_with_position() {
    let mut reader = BitReader::new(&[0xFF]);
    reader.skip_bits(8).unwrap();
    let err = reader.read_bit().unwrap_err();
    assert_eq!(
        err,
        BitstreamError::Truncated {
            at_bit: 8,
            needed_bits: 1,
            available_bits: 0,
        }
    );
    assert!(err.is_truncated());
    assert_eq!(err.at_byte(), 1);
}

#[test]
fn empty_buffer_yields_truncation_not_panic() {
    let mut reader = BitReader::new(&[]);
    assert!(reader.read_bit().unwrap_err().is_truncated());
    assert!(reader.read_ue().unwrap_err().is_truncated());
    assert!(reader.read_se().unwrap_err().is_truncated());
    assert!(!reader.has_more_rbsp_data());
}

/// ITU-T H.264 clause 9.1, table 9-1.
#[test]
fn unsigned_exp_golomb_matches_the_spec_table() {
    let cases = [
        ("1", 0),
        ("010", 1),
        ("011", 2),
        ("00100", 3),
        ("00101", 4),
        ("00110", 5),
        ("00111", 6),
        ("0001000", 7),
        ("0001111", 14),
        ("000010000", 15),
        ("000011111", 30),
    ];
    for (pattern, expected) in cases {
        let data = bits(pattern);
        let mut reader = BitReader::new(&data);
        assert_eq!(reader.read_ue().unwrap(), expected, "pattern {pattern}");
    }
}

#[test]
fn unsigned_exp_golomb_reads_consecutive_codes() {
    let data = bits("1_010_011_00100");
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_ue().unwrap(), 0);
    assert_eq!(reader.read_ue().unwrap(), 1);
    assert_eq!(reader.read_ue().unwrap(), 2);
    assert_eq!(reader.read_ue().unwrap(), 3);
}

/// The largest representable `ue(v)`: 31 leading zeros gives `2^32 - 2`.
#[test]
fn unsigned_exp_golomb_reaches_its_upper_bound() {
    let pattern = format!("{}1{}", "0".repeat(31), "1".repeat(31));
    let data = bits(&pattern);
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_ue().unwrap(), u32::MAX - 1);
}

/// 32 leading zeros cannot encode a representable value. Accepting it would let
/// a malformed stream drive an unbounded read (PRD N2.4).
#[test]
fn unsigned_exp_golomb_rejects_overlong_codes() {
    let pattern = format!("{}1{}", "0".repeat(32), "1".repeat(32));
    let data = bits(&pattern);
    let mut reader = BitReader::new(&data);
    assert_eq!(
        reader.read_ue(),
        Err(BitstreamError::Invalid {
            at_bit: 0,
            reason: InvalidReason::ExpGolombTooLong,
        })
    );
}

/// A run of zeros must terminate whether it hits the length bound or the end of
/// the buffer. Neither may hang.
#[test]
fn all_zero_buffers_terminate() {
    let mut long = BitReader::new(&[0u8; 16]);
    assert_eq!(
        long.read_ue(),
        Err(BitstreamError::Invalid {
            at_bit: 0,
            reason: InvalidReason::ExpGolombTooLong,
        })
    );

    let mut short = BitReader::new(&[0u8; 2]);
    assert!(short.read_ue().unwrap_err().is_truncated());
}

/// ITU-T H.264 clause 9.1.1, table 9-3.
#[test]
fn signed_exp_golomb_matches_the_spec_table() {
    let cases = [
        ("1", 0),
        ("010", 1),
        ("011", -1),
        ("00100", 2),
        ("00101", -2),
        ("00110", 3),
        ("00111", -3),
        ("0001000", 4),
        ("0001001", -4),
    ];
    for (pattern, expected) in cases {
        let data = bits(pattern);
        let mut reader = BitReader::new(&data);
        assert_eq!(reader.read_se().unwrap(), expected, "pattern {pattern}");
    }
}

#[test]
fn signed_exp_golomb_spans_its_full_range() {
    // codeNum 2^32 - 2 is even, mapping to the most negative representable value.
    let pattern = format!("{}1{}", "0".repeat(31), "1".repeat(31));
    let data = bits(&pattern);
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_se().unwrap(), -(i32::MAX));
}

/// ITU-T H.264 clause 9.1.1: with `cMax == 1` the element is one inverted bit.
#[test]
fn truncated_exp_golomb_inverts_a_single_bit() {
    let zero = bits("0");
    assert_eq!(BitReader::new(&zero).read_te(1).unwrap(), 1);

    let one = bits("1");
    assert_eq!(BitReader::new(&one).read_te(1).unwrap(), 0);
}

#[test]
fn truncated_exp_golomb_falls_back_to_ue() {
    let data = bits("00100");
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_te(7).unwrap(), 3);
}

#[test]
fn truncated_exp_golomb_consumes_nothing_when_only_one_value_is_possible() {
    let data = bits("1010");
    let mut reader = BitReader::new(&data);
    assert_eq!(reader.read_te(0).unwrap(), 0);
    assert_eq!(reader.bit_position(), 0);
}

/// ITU-T H.264 clause 7.2. `0xA8` is `1010_1000`: four payload bits, the
/// `rbsp_stop_one_bit` at index 4, then alignment zeros.
#[test]
fn more_rbsp_data_stops_at_the_trailing_bits() {
    let data = [0xA8];
    let mut reader = BitReader::new(&data);
    assert!(reader.has_more_rbsp_data());
    reader.skip_bits(3).unwrap();
    assert!(reader.has_more_rbsp_data());
    reader.skip_bits(1).unwrap();
    assert!(!reader.has_more_rbsp_data());
}

#[test]
fn more_rbsp_data_is_false_without_a_stop_bit() {
    let data = [0u8; 4];
    assert!(!BitReader::new(&data).has_more_rbsp_data());
}

/// ITU-T H.264 clause 7.3.2.11.
#[test]
fn reads_well_formed_rbsp_trailing_bits() {
    let data = [0xA8];
    let mut reader = BitReader::new(&data);
    reader.skip_bits(4).unwrap();
    reader.read_rbsp_trailing_bits().unwrap();
    assert!(reader.is_byte_aligned());
    assert_eq!(reader.bits_remaining(), 0);
}

#[test]
fn rejects_a_zero_rbsp_stop_bit() {
    let data = [0x00];
    let mut reader = BitReader::new(&data);
    assert_eq!(
        reader.read_rbsp_trailing_bits(),
        Err(BitstreamError::Invalid {
            at_bit: 0,
            reason: InvalidReason::MissingRbspStopBit,
        })
    );
}

#[test]
fn rejects_a_nonzero_rbsp_alignment_bit() {
    let data = [0b1100_0000];
    let mut reader = BitReader::new(&data);
    assert_eq!(
        reader.read_rbsp_trailing_bits(),
        Err(BitstreamError::Invalid {
            at_bit: 1,
            reason: InvalidReason::NonZeroRbspAlignmentBit,
        })
    );
}

#[test]
fn skip_past_the_end_is_refused_and_leaves_the_cursor_put() {
    let mut reader = BitReader::new(&[0xFF, 0xFF]);
    assert!(reader.skip_bits(17).unwrap_err().is_truncated());
    assert_eq!(reader.bit_position(), 0);
}

/// A deterministic stand-in for the fuzz target that arrives with the NAL layer
/// (PRD 8.3). Every operation on every input must return, and must leave the
/// cursor within the buffer.
#[test]
fn arbitrary_operations_on_arbitrary_data_never_panic() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..2_000 {
        let len = (next() % 24) as usize;
        let data: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
        let mut reader = BitReader::new(&data);
        let total_bits = (len as u64) * 8;

        for _ in 0..16 {
            match next() % 7 {
                0 => drop(reader.read_bit()),
                1 => drop(reader.read_bits((next() % 34) as u32)),
                2 => drop(reader.read_ue()),
                3 => drop(reader.read_se()),
                4 => drop(reader.read_te((next() % 4) as u32)),
                5 => drop(reader.skip_bits(next() % 40)),
                _ => drop(reader.read_rbsp_trailing_bits()),
            }
            assert!(
                reader.bit_position() <= total_bits,
                "cursor {} escaped a {total_bits}-bit buffer",
                reader.bit_position()
            );
            assert_eq!(
                reader.bits_remaining(),
                total_bits - reader.bit_position(),
                "remaining count disagrees with the cursor"
            );
        }
    }
}
