#[cfg(target_arch = "x86")]
use std::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;
use std::cmp::min;

use crate::num::{SignedVarIntTarget, VarIntTarget};
use crate::VarIntDecodeError;

mod lookup;

/// Decodes a single varint from the input slice. Requires SSSE3 support.
///
/// Produces a tuple containing the decoded number and the number of bytes read. For best
/// performance, provide a slice at least 16 bytes in length, or use the unsafe version directly.
///
/// # Examples
/// ```
/// use varint_simd::{decode, VarIntDecodeError};
///
/// fn main() -> Result<(), VarIntDecodeError> {
///     let decoded = decode::<u32>(&[185, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
///     assert_eq!(decoded, (1337, 2));
///     Ok(())
/// }
/// ```
#[inline]
#[cfg(any(target_feature = "ssse3", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "ssse3")))]
pub fn decode<T: VarIntTarget>(bytes: &[u8]) -> Result<(T, u8), VarIntDecodeError> {
    let result = if bytes.len() >= 16 {
        unsafe { decode_unsafe(bytes.as_ptr()) }
    } else if !bytes.is_empty() {
        let mut data = [0u8; 16];
        let len = min(10, bytes.len());
        data[..len].copy_from_slice(&bytes[..len]);
        unsafe { decode_unsafe(data.as_ptr()) }
    } else {
        return Err(VarIntDecodeError::NotEnoughBytes);
    };

    if result.1 > T::MAX_VARINT_BYTES
        || result.1 == T::MAX_VARINT_BYTES
            && bytes[(T::MAX_VARINT_BYTES - 1) as usize] > T::MAX_LAST_VARINT_BYTE
    {
        Err(VarIntDecodeError::Overflow)
    } else {
        Ok(result)
    }
}

/// Convenience function for decoding a single varint in ZigZag format from the input slice.
/// See also: [`decode`]
///
/// # Examples
/// ```
/// use varint_simd::{decode_zigzag, VarIntDecodeError};
///
/// fn main() -> Result<(), VarIntDecodeError> {
///     let decoded = decode_zigzag::<i32>(&[39, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
///     assert_eq!(decoded, (-20, 1));
///     Ok(())
/// }
/// ```
#[inline]
#[cfg(any(target_feature = "ssse3", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "ssse3")))]
pub fn decode_zigzag<T: SignedVarIntTarget>(bytes: &[u8]) -> Result<(T, u8), VarIntDecodeError> {
    decode::<T::Unsigned>(bytes).map(|r| (r.0.unzigzag(), r.1))
}

/// Decodes a single varint from the input pointer. Requires SSSE3 support.
///
/// # Safety
/// There must be at least 16 bytes of allocated memory after the beginning of the pointer.
/// Otherwise, there may be undefined behavior. Any data after the end of the varint are ignored.
/// A truncated value will be returned if the varint represents a number too large for the target
/// type.
///
/// You may prefer to use this unsafe interface if you know what you are doing and need a little
/// extra performance.
#[inline]
#[cfg(any(target_feature = "ssse3", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "ssse3")))]
pub unsafe fn decode_unsafe<T: VarIntTarget>(bytes: *const u8) -> (T, u8) {
    // It looks like you're trying to understand what this code does. You should probably read
    // this first: https://developers.google.com/protocol-buffers/docs/encoding#varints

    let b = _mm_loadu_si128(bytes as *const __m128i);

    // Get the most significant bits of each byte
    let bitmask: i32 = _mm_movemask_epi8(b);

    // A zero most significant bit indicates the end of a varint
    // Find how long the number really is
    let bm_not = !bitmask;
    let len = bm_not.trailing_zeros() + 1; // should compile to bsf or tzcnt (?), verify

    // Mask out irrelevant bytes from the vector
    let ascend = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    let mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(len as i8));
    let varint_part = _mm_and_si128(b, mask);

    // // Turn off the most significant bits
    // let msb_masked = _mm_and_si128(
    //     varint_part,
    //     _mm_set_epi8(
    //         0, 0, 0, 0, 0, 0, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
    //     ),
    // );

    // Turn the vector into a scalar value by concatenating the 7-bit values
    let num = T::vector_to_num(std::mem::transmute(varint_part)); // specialized functions for different number sizes

    (num, len as u8)
}

/// Decodes two varints simultaneously. Target types must fit within 16 bytes when varint encoded.
/// Requires SSSE3 support.
///
/// For example, it is permissible to decode `u32` and `u32`, and `u64` and `u32`, but it is not
/// possible to decode two `u64` values with this function simultaneously.
///
/// For best performance, ensure each target type is `u32` or smaller.
///
/// # Safety
/// There must be at least 16 bytes of allocated memory after the start of the pointer. Otherwise,
/// there may be undefined behavior. Any data after the two varints are ignored. Truncated values
/// will be returned if a varint exceeds the target type's limit.
#[inline]
#[cfg(any(target_feature = "ssse3", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "ssse3")))]
pub unsafe fn decode_two_unsafe<T: VarIntTarget, U: VarIntTarget>(
    bytes: *const u8,
) -> (T, u8, U, u8) {
    if T::MAX_VARINT_BYTES + U::MAX_VARINT_BYTES > 16 {
        // check will be eliminated at compile time
        panic!(
            "exceeded length limit: cannot decode {} and {}, total length {} exceeds 16 bytes",
            std::any::type_name::<T>(),
            std::any::type_name::<U>(),
            T::MAX_VARINT_BYTES + U::MAX_VARINT_BYTES
        );
    }

    let b = _mm_loadu_si128(bytes as *const __m128i);

    // First find where the boundaries are
    let bitmask = _mm_movemask_epi8(b) as u32;

    // Find the number of bytes taken up by each varint
    let bm_not = !bitmask;
    let first_len = bm_not.trailing_zeros() + 1; // should compile to bsf or tzcnt
    let bm_not_2 = bm_not >> first_len;
    let second_len = bm_not_2.trailing_zeros() + 1;

    let ascend = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

    let first_len_vec = _mm_set1_epi8(first_len as i8);
    let first_mask = _mm_cmplt_epi8(ascend, first_len_vec);
    let first = _mm_and_si128(b, first_mask);

    let second_shuf = _mm_add_epi8(ascend, first_len_vec);
    let second_shuffled = _mm_shuffle_epi8(b, second_shuf);
    let second_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(second_len as i8));
    let second = _mm_and_si128(second_shuffled, second_mask);

    let first_num;
    let second_num;

    // Only use "turbo" mode if the numbers fit in 64-bit lanes
    let should_turbo = T::MAX_VARINT_BYTES <= 8
        && U::MAX_VARINT_BYTES <= 8
        // PDEP/PEXT are still a little faster here
        && cfg!(not(all(
            target_arch = "x86_64",
            target_feature = "bmi2",
            fast_pdep
        )));
    if should_turbo {
        // const, so optimized out
        let comb = _mm_or_si128(first, _mm_bslli_si128(second, 8));

        let x = if T::MAX_VARINT_BYTES <= 2 && U::MAX_VARINT_BYTES <= 2 {
            dual_u8_stage2(comb)
        } else if T::MAX_VARINT_BYTES <= 3 && U::MAX_VARINT_BYTES <= 3 {
            dual_u16_stage2(comb)
        } else {
            dual_u32_stage2(comb)
        };

        let x: [u32; 4] = std::mem::transmute(x);
        // _mm_extract_epi32 requires SSE4.1
        first_num = T::cast_u32(x[0]);
        second_num = U::cast_u32(x[2]);
    } else {
        first_num = T::vector_to_num(std::mem::transmute(first));
        second_num = U::vector_to_num(std::mem::transmute(second));
    }

    (first_num, first_len as u8, second_num, second_len as u8)
}

#[inline(always)]
unsafe fn dual_u8_stage2(comb: __m128i) -> __m128i {
    _mm_or_si128(
        _mm_and_si128(comb, _mm_set_epi64x(0x000000000000007f, 0x000000000000007f)),
        _mm_srli_epi64(
            _mm_and_si128(comb, _mm_set_epi64x(0x0000000000000100, 0x0000000000000100)),
            1,
        ),
    )
}

#[inline(always)]
unsafe fn dual_u16_stage2(comb: __m128i) -> __m128i {
    _mm_or_si128(
        _mm_or_si128(
            _mm_and_si128(comb, _mm_set_epi64x(0x000000000000007f, 0x000000000000007f)),
            _mm_srli_epi64(
                _mm_and_si128(comb, _mm_set_epi64x(0x0000000000030000, 0x0000000000030000)),
                2,
            ),
        ),
        _mm_srli_epi64(
            _mm_and_si128(comb, _mm_set_epi64x(0x0000000000007f00, 0x0000000000007f00)),
            1,
        ),
    )
}

#[inline(always)]
unsafe fn dual_u32_stage2(comb: __m128i) -> __m128i {
    _mm_or_si128(
        _mm_or_si128(
            _mm_and_si128(comb, _mm_set_epi64x(0x000000000000007f, 0x000000000000007f)),
            _mm_srli_epi64(
                _mm_and_si128(comb, _mm_set_epi64x(0x0000000f00000000, 0x0000000f00000000)),
                4,
            ),
        ),
        _mm_or_si128(
            _mm_or_si128(
                _mm_srli_epi64(
                    _mm_and_si128(comb, _mm_set_epi64x(0x000000007f000000, 0x000000007f000000)),
                    3,
                ),
                _mm_srli_epi64(
                    _mm_and_si128(comb, _mm_set_epi64x(0x00000000007f0000, 0x00000000007f0000)),
                    2,
                ),
            ),
            _mm_srli_epi64(
                _mm_and_si128(comb, _mm_set_epi64x(0x0000000000007f00, 0x0000000000007f00)),
                1,
            ),
        ),
    )
}

/// Decode two adjacent varints simultaneously from the input pointer. Requires AVX2. Allows for
/// decoding a pair of `u64` values. For smaller values, the non-wide variation of this function
/// will probably be faster.
///
/// # Safety
/// There must be at least 32 bytes of allocated memory after the beginning of the pointer.
/// Otherwise, there may be undefined behavior. Calling code should ensure that AVX2 is supported
/// before calling this function.
#[inline]
#[cfg(any(target_feature = "avx2", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "avx2")))]
pub unsafe fn decode_two_wide_unsafe<T: VarIntTarget, U: VarIntTarget>(
    bytes: *const u8,
) -> (T, u8, U, u8) {
    let b = _mm256_loadu_si256(bytes as *const __m256i);

    // Get the most significant bits
    let bitmask = _mm256_movemask_epi8(b) as u32;

    // Find the number of bytes taken up by each varint
    let bm_not = !bitmask;
    let first_len = bm_not.trailing_zeros() + 1; // should compile to bsf or tzcnt
    let bm_not_2 = bm_not >> first_len;
    let second_len = bm_not_2.trailing_zeros() + 1;

    // Create and parse vector consisting solely of the first varint
    let ascend = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    let first_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(first_len as i8));
    let first = _mm_and_si128(_mm256_extracti128_si256(b, 0), first_mask);

    // The second is much more tricky.
    let shuf_gen = _mm256_setr_epi8(
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
        12, 13, 14, 15,
    );

    // Rearrange each 128-bit lane such that ORing them together results in the window of data we want)
    let shuf_add = _mm256_set_m128i(
        _mm_set1_epi8(-(16i8 - first_len as i8)),
        _mm_set1_epi8(first_len as i8),
    );
    let shuf_added = _mm256_add_epi8(shuf_gen, shuf_add);
    let shuf = _mm256_or_si256(
        shuf_added,
        _mm256_cmpgt_epi8(shuf_added, _mm256_set1_epi8(15)),
    );
    let shuffled = _mm256_shuffle_epi8(b, shuf);

    // OR the halves together, and now we have a view of the second varint
    let second_shifted = _mm_or_si128(
        _mm256_extracti128_si256(shuffled, 0),
        _mm256_extracti128_si256(shuffled, 1),
    );
    let second_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(second_len as i8));
    let second = _mm_and_si128(second_shifted, second_mask);

    let first_num;
    let second_num;

    // PEXT on the two halves is still slower, at least on Coffee Lake and Broadwell
    let should_turbo = true;
    if should_turbo {
        // Decode the two halves in parallel using SSE2
        let comb_lo = _mm_unpacklo_epi64(first, second);
        let x_lo = _mm_or_si128(
            _mm_or_si128(
                _mm_or_si128(
                    _mm_and_si128(comb_lo, _mm_set1_epi64x(0x000000000000007f)),
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x7f00000000000000)),
                        7,
                    ),
                ),
                _mm_or_si128(
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x007f000000000000)),
                        6,
                    ),
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x00007f0000000000)),
                        5,
                    ),
                ),
            ),
            _mm_or_si128(
                _mm_or_si128(
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x0000007f00000000)),
                        4,
                    ),
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x000000007f000000)),
                        3,
                    ),
                ),
                _mm_or_si128(
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x00000000007f0000)),
                        2,
                    ),
                    _mm_srli_epi64(
                        _mm_and_si128(comb_lo, _mm_set1_epi64x(0x0000000000007f00)),
                        1,
                    ),
                ),
            ),
        );

        let comb_hi = _mm_unpackhi_epi64(first, second);
        let x_hi = _mm_or_si128(
            _mm_slli_epi64(
                _mm_and_si128(comb_hi, _mm_set1_epi64x(0x0000000000000100)),
                55,
            ),
            _mm_slli_epi64(
                _mm_and_si128(comb_hi, _mm_set1_epi64x(0x000000000000007f)),
                56,
            ),
        );

        let x = _mm_or_si128(x_lo, x_hi);

        first_num = T::cast_u64(_mm_extract_epi64(x, 0) as u64);
        second_num = U::cast_u64(_mm_extract_epi64(x, 2) as u64);
    } else {
        first_num = T::vector_to_num(std::mem::transmute(first));
        second_num = U::vector_to_num(std::mem::transmute(second));
    }

    (first_num, first_len as u8, second_num, second_len as u8)
}

/// Decodes four varints simultaneously. Target types must fit within 16 bytes when varint encoded.
/// Requires SSSE3 support.
///
/// For best performance, ensure each target type is `u16` or smaller.
///
/// # Safety
/// There must be at least 16 bytes of allocated memory after the start of the pointer. Otherwise,
/// there may be undefined behavior. Any data after the four varints are ignored. Truncated values
/// will be returned if a varint exceeds the target type's limit.
#[inline]
#[cfg(any(target_feature = "ssse3", doc))]
#[cfg_attr(rustc_nightly, doc(cfg(target_feature = "ssse3")))]
pub unsafe fn decode_four_unsafe<
    T: VarIntTarget,
    U: VarIntTarget,
    V: VarIntTarget,
    W: VarIntTarget,
>(
    bytes: *const u8,
) -> (T, U, V, W, u8, u8, u8, u8) {
    if T::MAX_VARINT_BYTES + U::MAX_VARINT_BYTES + V::MAX_VARINT_BYTES + W::MAX_VARINT_BYTES > 16 {
        // check will be eliminated at compile time
        panic!(
            "exceeded length limit: cannot decode {}, {}, {}, and {}, total length {} exceeds 16 bytes",
            std::any::type_name::<T>(),
            std::any::type_name::<U>(),
            std::any::type_name::<V>(),
            std::any::type_name::<W>(),
            T::MAX_VARINT_BYTES + U::MAX_VARINT_BYTES + V::MAX_VARINT_BYTES + W::MAX_VARINT_BYTES
        );
    }

    let b = _mm_loadu_si128(bytes as *const __m128i);

    // First find where the boundaries are
    let bitmask = _mm_movemask_epi8(b) as u32;

    // Find the number of bytes taken up by each varint
    let bm_not = !bitmask;
    let first_len = bm_not.trailing_zeros() + 1; // should compile to bsf or tzcnt
    let bm_not_2 = bm_not >> first_len;
    let second_len = bm_not_2.trailing_zeros() + 1;
    let bm_not_3 = bm_not_2 >> second_len;
    let third_len = bm_not_3.trailing_zeros() + 1;
    let bm_not_4 = bm_not_3 >> third_len;
    let fourth_len = bm_not_4.trailing_zeros() + 1;

    let ascend = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);

    let first_len_vec = _mm_set1_epi8(first_len as i8);
    let first_mask = _mm_cmplt_epi8(ascend, first_len_vec);
    let first = _mm_and_si128(b, first_mask);

    let second_shuf = _mm_add_epi8(ascend, first_len_vec);
    let second_shuffled = _mm_shuffle_epi8(b, second_shuf);
    let second_len_vec = _mm_set1_epi8(second_len as i8);
    let second_mask = _mm_cmplt_epi8(ascend, second_len_vec);
    let second = _mm_and_si128(second_shuffled, second_mask);

    let third_shuf = _mm_add_epi8(ascend, second_len_vec);
    let third_shuffled = _mm_shuffle_epi8(second_shuffled, third_shuf);
    let third_len_vec = _mm_set1_epi8(third_len as i8);
    let third_mask = _mm_cmplt_epi8(ascend, third_len_vec);
    let third = _mm_and_si128(third_shuffled, third_mask);

    let fourth_shuf = _mm_add_epi8(ascend, third_len_vec);
    let fourth_shuffled = _mm_shuffle_epi8(third_shuffled, fourth_shuf);
    let fourth_len_vec = _mm_set1_epi8(fourth_len as i8);
    let fourth_mask = _mm_cmplt_epi8(ascend, fourth_len_vec);
    let fourth = _mm_and_si128(fourth_shuffled, fourth_mask);

    let first_num;
    let second_num;
    let third_num;
    let fourth_num;

    // Only use "turbo" mode if the numbers fit in 64-bit lanes
    let should_turbo = T::MAX_VARINT_BYTES <= 4
        && U::MAX_VARINT_BYTES <= 4
        && V::MAX_VARINT_BYTES <= 4
        && W::MAX_VARINT_BYTES <= 4
        // PDEP/PEXT are still a little faster here
        && cfg!(not(all(
            target_arch = "x86_64",
            target_feature = "bmi2",
            fast_pdep
        )));
    if should_turbo {
        // const, so optimized out
        let comb = _mm_or_si128(
            _mm_or_si128(first, _mm_bslli_si128(second, 4)),
            _mm_or_si128(_mm_bslli_si128(third, 8), _mm_bslli_si128(fourth, 12)),
        );

        let x = if T::MAX_VARINT_BYTES <= 2 && U::MAX_VARINT_BYTES <= 2 {
            _mm_or_si128(
                _mm_and_si128(comb, _mm_set1_epi32(0x0000007f)),
                _mm_srli_epi32(_mm_and_si128(comb, _mm_set1_epi32(0x00000100)), 1),
            )
        } else {
            _mm_or_si128(
                _mm_or_si128(
                    _mm_and_si128(comb, _mm_set1_epi32(0x0000007f)),
                    _mm_srli_epi32(_mm_and_si128(comb, _mm_set1_epi32(0x00030000)), 2),
                ),
                _mm_srli_epi32(_mm_and_si128(comb, _mm_set1_epi32(0x00007f00)), 1),
            )
        };

        let x: [u32; 4] = std::mem::transmute(x);
        // _mm_extract_epi32 requires SSE4.1
        first_num = T::cast_u32(x[0]);
        second_num = U::cast_u32(x[1]);
        third_num = V::cast_u32(x[2]);
        fourth_num = W::cast_u32(x[3]);
    } else {
        first_num = T::vector_to_num(std::mem::transmute(first));
        second_num = U::vector_to_num(std::mem::transmute(second));
        third_num = V::vector_to_num(std::mem::transmute(third));
        fourth_num = W::vector_to_num(std::mem::transmute(fourth));
    }

    (
        first_num,
        second_num,
        third_num,
        fourth_num,
        first_len as u8,
        second_len as u8,
        third_len as u8,
        fourth_len as u8,
    )
}

/// **Experimental.** Decodes three adjacent varints from the given pointer simultaneously.
/// This currently runs much slower than a scalar or hybrid implementation. Requires AVX2 support.
///
/// # Safety
/// There must be at least 32 bytes of memory allocated after the beginning of the pointer.
/// Otherwise, there may be undefined behavior.
#[inline]
#[cfg(target_feature = "avx2")]
pub unsafe fn decode_three_unsafe<T: VarIntTarget, U: VarIntTarget, V: VarIntTarget>(
    bytes: &[u8],
) -> (T, u8, U, u8, V, u8) {
    let b = _mm256_loadu_si256(bytes.as_ptr() as *const __m256i);

    // Get the most significant bits
    let bitmask = _mm256_movemask_epi8(b) as u32;

    // Find the number of bytes taken up by each varint
    let bm_not = !bitmask;
    let first_len = bm_not.trailing_zeros() + 1; // should compile to bsf or tzcnt (?), verify
    let bm_not_2 = bm_not >> first_len;
    let second_len = bm_not_2.trailing_zeros() + 1;
    let bm_not_3 = bm_not_2 >> second_len;
    let third_len = bm_not_3.trailing_zeros() + 1;

    // println!("{} {} {}", first_len, second_len, third_len);

    // Create and parse vector consisting solely of the first varint
    let ascend = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    let first_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(first_len as i8));
    let first = _mm_and_si128(_mm256_extracti128_si256(b, 0), first_mask);
    // println!("{:?}", slice_m128i(first));

    let msb_mask = _mm_set_epi8(
        0, 0, 0, 0, 0, 0, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
    );
    let first_msb = _mm_and_si128(msb_mask, first);
    let first_result = T::vector_to_num(std::mem::transmute(first_msb));

    // The second and third are much more tricky.
    let shuf_gen = _mm256_setr_epi8(
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
        12, 13, 14, 15,
    );

    // Rearrange each 128-bit lane such that ORing them together results in the window of data we want)
    let shuf_add = _mm256_set_m128i(
        _mm_set1_epi8(-(16i8 - first_len as i8)),
        _mm_set1_epi8(first_len as i8),
    );
    let shuf_added = _mm256_add_epi8(shuf_gen, shuf_add);
    let shuf = _mm256_or_si256(
        shuf_added,
        _mm256_cmpgt_epi8(shuf_added, _mm256_set1_epi8(15)),
    );
    let shuffled = _mm256_shuffle_epi8(b, shuf);

    // OR the halves together, and now we have a view of the second varint
    let second_shifted = _mm_or_si128(
        _mm256_extracti128_si256(shuffled, 0),
        _mm256_extracti128_si256(shuffled, 1),
    );
    let second_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(second_len as i8));
    let second = _mm_and_si128(second_shifted, second_mask);
    // println!("second {:?}", slice_m128i(second));

    // Mask out the MSB, and we're done
    let second_msb = _mm_and_si128(msb_mask, second);
    let second_result = U::vector_to_num(std::mem::transmute(second_msb));

    // The third is done similarly
    let shuf_add = _mm256_set_m128i(
        _mm_set1_epi8(-(16i8 - (first_len + second_len) as i8)),
        _mm_set1_epi8((first_len + second_len) as i8),
    );
    let shuf_added = _mm256_add_epi8(shuf_gen, shuf_add);
    let shuf = _mm256_or_si256(
        shuf_added,
        _mm256_cmpgt_epi8(shuf_added, _mm256_set1_epi8(15)),
    );
    let shuffled = _mm256_shuffle_epi8(b, shuf);

    let third_shifted = _mm_or_si128(
        _mm256_extracti128_si256(shuffled, 0),
        _mm256_extracti128_si256(shuffled, 1),
    );
    let third_mask = _mm_cmplt_epi8(ascend, _mm_set1_epi8(third_len as i8));
    let third = _mm_and_si128(third_mask, third_shifted);
    // println!("third {:?}", slice_m128i(third));

    let third_msb = _mm_and_si128(msb_mask, third);
    let third_result = V::vector_to_num(std::mem::transmute(third_msb));

    (
        first_result,
        first_len as u8,
        second_result,
        second_len as u8,
        third_result,
        third_len as u8,
    )
}