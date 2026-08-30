//! A fallible, bit-level cursor over an RBSP buffer.

use crate::error::{BitstreamError, InvalidReason, Result};

/// The largest number of leading zero bits an Exp-Golomb code may have.
///
/// ITU-T H.264 clause 9.1 gives `ue(v)` the range `[0, 2^32 - 2]`. With `n`
/// leading zeros the decoded value is `2^n - 1 + read_bits(n)`, so `n = 31`
/// yields exactly `2^32 - 2` and `n = 32` cannot be represented.
const MAX_LEADING_ZEROS: u32 = 31;

/// Reads bits, and the syntax element encodings built on them, from a buffer.
///
/// The buffer is expected to be an RBSP: a NAL unit payload with emulation
/// prevention bytes already removed. Nothing here interprets the bits — that is
/// the codec's job — but everything here refuses to read past the end of the
/// buffer, loop unboundedly, or overflow.
///
/// # Examples
///
/// ```
/// use h264_bitstream::BitReader;
///
/// // 0b1_010_011 — the Exp-Golomb codes for 0, 1 and 2.
/// let mut r = BitReader::new(&[0b1_010_011_0]);
/// assert_eq!(r.read_ue()?, 0);
/// assert_eq!(r.read_ue()?, 1);
/// assert_eq!(r.read_ue()?, 2);
/// # Ok::<(), h264_bitstream::BitstreamError>(())
/// ```
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bits consumed so far, from the start of `data`.
    pos: u64,
    /// Total bits in `data`.
    len: u64,
    /// Absolute index of the final set bit, which in a well-formed RBSP is the
    /// `rbsp_stop_one_bit`. Computed once so that [`BitReader::has_more_rbsp_data`]
    /// is O(1) — parsers call it in a loop, and an O(n) implementation would
    /// make scaling-list and SEI parsing quadratic in the size of the payload.
    stop_bit: Option<u64>,
}

impl<'a> BitReader<'a> {
    /// Creates a reader over `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            len: (data.len() as u64).saturating_mul(8),
            stop_bit: Self::find_stop_bit(data),
        }
    }

    /// Locates the last set bit in `data`.
    ///
    /// Scanning backwards means this normally stops at the final byte, since a
    /// well-formed RBSP ends with the stop bit followed only by alignment zeros.
    fn find_stop_bit(data: &[u8]) -> Option<u64> {
        let (index, byte) = data.iter().enumerate().rev().find(|&(_, b)| *b != 0)?;
        // `byte` is non-zero, so `trailing_zeros()` is in 0..=7; the saturating
        // subtraction makes that bound explicit rather than assumed.
        let bit_in_byte = u64::from(7_u32.saturating_sub(byte.trailing_zeros()));
        Some((index as u64).saturating_mul(8).saturating_add(bit_in_byte))
    }

    /// Bits consumed so far.
    #[must_use]
    pub fn bit_position(&self) -> u64 {
        self.pos
    }

    /// Bits not yet consumed.
    #[must_use]
    pub fn bits_remaining(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }

    /// Whether the cursor sits on a byte boundary.
    #[must_use]
    pub fn is_byte_aligned(&self) -> bool {
        self.pos.is_multiple_of(8)
    }

    fn truncated(&self, needed_bits: u32) -> BitstreamError {
        BitstreamError::Truncated {
            at_bit: self.pos,
            needed_bits,
            available_bits: self.bits_remaining(),
        }
    }

    fn invalid(&self, at_bit: u64, reason: InvalidReason) -> BitstreamError {
        BitstreamError::Invalid { at_bit, reason }
    }

    /// Reads a single bit — ITU-T H.264 clause 7.2, `u(1)`.
    ///
    /// # Errors
    ///
    /// Returns [`BitstreamError::Truncated`] at the end of the buffer.
    pub fn read_bit(&mut self) -> Result<bool> {
        let byte_index = usize::try_from(self.pos / 8).map_err(|_| self.truncated(1))?;
        let byte = *self.data.get(byte_index).ok_or_else(|| self.truncated(1))?;
        // `pos % 8` is in 0..=7, so the mask selects one bit, MSB first.
        let mask = 0x80_u8 >> (self.pos % 8);
        self.pos = self.pos.saturating_add(1);
        Ok(byte & mask != 0)
    }

    /// Reads `n` bits as an unsigned value — ITU-T H.264 clause 7.2, `u(n)`.
    ///
    /// This also covers `f(n)`, which has identical parsing; only the semantics
    /// of the result differ.
    ///
    /// # Errors
    ///
    /// Returns [`BitstreamError::Truncated`] if fewer than `n` bits remain, and
    /// [`InvalidReason::BitCountOutOfRange`] if `n` exceeds 32.
    pub fn read_bits(&mut self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(self.invalid(self.pos, InvalidReason::BitCountOutOfRange));
        }
        if u64::from(n) > self.bits_remaining() {
            return Err(self.truncated(n));
        }
        let mut value: u32 = 0;
        for _ in 0..n {
            // Cannot overflow: the loop runs at most 32 times and `value` holds
            // at most `n - 1` significant bits when the shift happens.
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    /// Advances the cursor by `n` bits without decoding them.
    ///
    /// # Errors
    ///
    /// Returns [`BitstreamError::Truncated`] if fewer than `n` bits remain.
    pub fn skip_bits(&mut self, n: u64) -> Result<()> {
        if n > self.bits_remaining() {
            return Err(self.truncated(u32::try_from(n).unwrap_or(u32::MAX)));
        }
        self.pos = self.pos.saturating_add(n);
        Ok(())
    }

    /// Reads an unsigned Exp-Golomb code — ITU-T H.264 clause 9.1, `ue(v)`.
    ///
    /// # Errors
    ///
    /// Returns [`BitstreamError::Truncated`] if the code runs off the end, and
    /// [`InvalidReason::ExpGolombTooLong`] if it begins with 32 or more zero
    /// bits, which cannot encode a representable value.
    pub fn read_ue(&mut self) -> Result<u32> {
        let start = self.pos;
        let mut leading_zeros: u32 = 0;
        while !self.read_bit()? {
            leading_zeros = leading_zeros.saturating_add(1);
            if leading_zeros > MAX_LEADING_ZEROS {
                return Err(self.invalid(start, InvalidReason::ExpGolombTooLong));
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zeros)?;
        // `leading_zeros <= 31`, so `1 << leading_zeros` is at most `2^31` and
        // the sum is at most `2^32 - 2`. Written checked regardless.
        let base = (1_u32 << leading_zeros).saturating_sub(1);
        Ok(base.saturating_add(suffix))
    }

    /// Reads a signed Exp-Golomb code — ITU-T H.264 clause 9.1.1, `se(v)`.
    ///
    /// # Errors
    ///
    /// As [`BitReader::read_ue`].
    pub fn read_se(&mut self) -> Result<i32> {
        let code_num = self.read_ue()?;
        // Clause 9.1.1: value = (-1)^(k+1) * ceil(k / 2). `code_num` is at most
        // `2^32 - 2`, so the magnitude is at most `2^31 - 1` and always fits i32.
        let magnitude = i64::from(code_num.div_ceil(2));
        let value = if code_num.is_multiple_of(2) {
            magnitude.saturating_neg()
        } else {
            magnitude
        };
        i32::try_from(value).map_err(|_| self.invalid(self.pos, InvalidReason::ExpGolombTooLong))
    }

    /// Reads a truncated Exp-Golomb code — ITU-T H.264 clause 9.1.1, `te(v)`.
    ///
    /// `max_value` is the largest value the syntax element may take (`cMax` in
    /// the specification), which the caller derives from context.
    ///
    /// When `cMax` is 1 the element is a single inverted bit; otherwise it is a
    /// plain `ue(v)`. The `cMax == 0` case is not reachable from valid H.264
    /// syntax, since every use of `te(v)` is guarded by a condition that makes
    /// the element absent when only one value is possible. It is handled the way
    /// FFmpeg handles it — yield 0 and consume no bits — so that a stream which
    /// somehow reaches it decodes the same way here as in the reference tools.
    ///
    /// # Errors
    ///
    /// As [`BitReader::read_ue`].
    pub fn read_te(&mut self, max_value: u32) -> Result<u32> {
        match max_value {
            0 => Ok(0),
            1 => Ok(u32::from(!self.read_bit()?)),
            _ => self.read_ue(),
        }
    }

    /// Whether any RBSP data precedes the trailing bits — ITU-T H.264 clause 7.2,
    /// `more_rbsp_data()`.
    ///
    /// A well-formed RBSP ends with an `rbsp_stop_one_bit` followed by zero or
    /// more alignment zeros, so the final set bit in the buffer marks the end of
    /// the payload. There is more data exactly while the cursor is before it.
    #[must_use]
    pub fn has_more_rbsp_data(&self) -> bool {
        match self.stop_bit {
            // No set bit anywhere: the payload is malformed and holds nothing
            // that can be parsed, so report no further data rather than looping.
            None => false,
            Some(stop) => self.pos < stop,
        }
    }

    /// Consumes the RBSP trailing bits — ITU-T H.264 clause 7.3.2.11.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidReason::MissingRbspStopBit`] if the stop bit is zero,
    /// [`InvalidReason::NonZeroRbspAlignmentBit`] if an alignment bit is one,
    /// and [`BitstreamError::Truncated`] if the buffer ends first.
    pub fn read_rbsp_trailing_bits(&mut self) -> Result<()> {
        let start = self.pos;
        if !self.read_bit()? {
            return Err(self.invalid(start, InvalidReason::MissingRbspStopBit));
        }
        while !self.is_byte_aligned() {
            let at = self.pos;
            if self.read_bit()? {
                return Err(self.invalid(at, InvalidReason::NonZeroRbspAlignmentBit));
            }
        }
        Ok(())
    }
}
