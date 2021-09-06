//! SHA-512 `x86`/`x86_64` backend

#![allow(clippy::many_single_char_names)]

use core::mem::size_of;

#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

use crate::consts::{K64, K64X4};

cpufeatures::new!(avx2_cpuid, "avx", "avx2", "sse2", "sse3");

pub fn compress(state: &mut [u64; 8], blocks: &[[u8; 128]]) {
    // TODO: Replace with https://github.com/rust-lang/rfcs/pull/2725
    // after stabilization
    if avx2_cpuid::get() {
        unsafe {
            sha512_compress_x86_64_avx2(state, blocks);
        }
    } else {
        super::soft::compress(state, blocks);
    }
}

#[target_feature(enable = "avx,avx2,sse2,sse3")]
unsafe fn sha512_compress_x86_64_avx2(state: &mut [u64; 8], blocks: &[[u8; 128]]) {
    let mut start_block = 0;

    if blocks.len() & 0b1 != 0 {
        sha512_compress_x86_64_avx(state, &blocks[0]);
        start_block += 1;
    }

    let mut ms: MsgSchedule = Default::default();
    let mut t2: RoundStates = [0u64; SHA512_ROUNDS_NUM];
    let mut x = [_mm256_setzero_si256(); 8];

    for i in (start_block..blocks.len()).step_by(2) {
        load_data_avx2(&mut x, &mut ms, &mut t2, blocks.as_ptr().add(i) as *const _);

        // First block
        let mut current_state = *state;
        rounds_0_63_avx2(&mut current_state, &mut x, &mut ms, &mut t2);
        rounds_64_79(&mut current_state, &ms);
        accumulate_state(state, &current_state);

        // Second block
        current_state = *state;
        process_second_block(&mut current_state, &t2);
        accumulate_state(state, &current_state);
    }
}

#[inline(always)]
unsafe fn sha512_compress_x86_64_avx(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut ms = Default::default();
    let mut x = [_mm_setzero_si128(); 8];

    // Reduced to single iteration
    let mut current_state = *state;
    load_data_avx(&mut x, &mut ms, block.as_ptr() as *const _);
    rounds_0_63_avx(&mut current_state, &mut x, &mut ms);
    rounds_64_79(&mut current_state, &ms);
    accumulate_state(state, &current_state);
}

#[inline(always)]
unsafe fn load_data_avx(x: &mut [__m128i; 8], ms: &mut MsgSchedule, data: *const __m128i) {
    #[allow(non_snake_case)]
    let MASK = _mm_setr_epi32(0x04050607, 0x00010203, 0x0c0d0e0f, 0x08090a0b);

    macro_rules! unrolled_iterations {
        ($($i:literal),*) => {$(
            x[$i] = _mm_loadu_si128(data.add($i) as *const _);
            x[$i] = _mm_shuffle_epi8(x[$i], MASK);

            let y = _mm_add_epi64(
                x[$i],
                _mm_loadu_si128(&K64[2 * $i] as *const u64 as *const _),
            );

            _mm_store_si128(&mut ms[2 * $i] as *mut u64 as *mut _, y);
        )*};
    }

    unrolled_iterations!(0, 1, 2, 3, 4, 5, 6, 7);
}

#[inline(always)]
unsafe fn load_data_avx2(
    x: &mut [__m256i; 8],
    ms: &mut MsgSchedule,
    t2: &mut RoundStates,
    data: *const __m128i,
) {
    #[allow(non_snake_case)]
    let MASK = _mm256_set_epi64x(
        0x0809_0A0B_0C0D_0E0F_i64,
        0x0001_0203_0405_0607_i64,
        0x0809_0A0B_0C0D_0E0F_i64,
        0x0001_0203_0405_0607_i64,
    );

    macro_rules! unrolled_iterations {
        ($($i:literal),*) => {$(
            x[$i] = _mm256_insertf128_si256::<1>(x[$i], _mm_loadu_si128(data.add($i) as *const _));
            x[$i] = _mm256_insertf128_si256::<0>(x[$i], _mm_loadu_si128(data.add($i + 1) as *const _));

            x[$i] = _mm256_shuffle_epi8(x[$i], MASK);
            let y = _mm256_add_epi64(
                x[$i],
                _mm256_loadu_si256(&K64X4[4 * $i] as *const u64 as *const _),
            );

            _mm_store_si128(
                &mut ms[2 * $i] as *mut u64 as *mut _,
                _mm256_extracti128_si256::<0>(y),
            );
            _mm_store_si128(
                &mut t2[2 * $i] as *mut u64 as *mut _,
                _mm256_extracti128_si256::<1>(y),
            );
        )*};
    }

    unrolled_iterations!(0, 1, 2, 3, 4, 5, 6, 7);
}

#[inline(always)]
unsafe fn rounds_0_63_avx(current_state: &mut State, x: &mut [__m128i; 8], ms: &mut MsgSchedule) {
    let mut k64_idx: usize = SHA512_BLOCK_WORDS_NUM;

    for _ in 0..4 {
        for j in 0..8 {
            let y = sha512_update_x_avx(x, &K64[k64_idx] as *const u64 as *const _);

            sha_round(current_state, ms[2 * j]);
            sha_round(current_state, ms[2 * j + 1]);

            _mm_store_si128(&mut ms[2 * j] as *const u64 as *mut _, y);
            k64_idx += 2;
        }
    }
}

#[inline(always)]
unsafe fn rounds_0_63_avx2(
    current_state: &mut State,
    x: &mut [__m256i; 8],
    ms: &mut MsgSchedule,
    t2: &mut RoundStates,
) {
    let mut k64x4_idx: usize = 2 * SHA512_BLOCK_WORDS_NUM;

    for i in 1..5 {
        for j in 0..8 {
            let y = sha512_update_x_avx2(x, &K64X4[k64x4_idx] as *const u64 as *const _);

            sha_round(current_state, ms[2 * j]);
            sha_round(current_state, ms[2 * j + 1]);

            _mm_store_si128(
                &mut ms[2 * j] as *mut u64 as *mut _,
                _mm256_extracti128_si256::<0>(y),
            );
            _mm_store_si128(
                &mut t2[(16 * i) + 2 * j] as *mut u64 as *mut _,
                _mm256_extracti128_si256::<1>(y),
            );

            k64x4_idx += 4;
        }
    }
}

#[inline(always)]
unsafe fn rounds_64_79(current_state: &mut State, ms: &MsgSchedule) {
    for i in 64..80 {
        sha_round(current_state, ms[i & 0xf]);
    }
}

#[inline(always)]
unsafe fn process_second_block(current_state: &mut State, t2: &RoundStates) {
    for t2 in t2 {
        sha_round(current_state, *t2);
    }
}

#[inline(always)]
unsafe fn sha_round(s: &mut State, x: u64) {
    macro_rules! big_sigma0 {
        ($a:expr) => {
            $a.rotate_right(28) ^ $a.rotate_right(34) ^ $a.rotate_right(39)
        };
    }
    macro_rules! big_sigma1 {
        ($a:expr) => {
            $a.rotate_right(14) ^ $a.rotate_right(18) ^ $a.rotate_right(41)
        };
    }
    macro_rules! bool3ary_202 {
        ($a:expr, $b:expr, $c:expr) => {
            $c ^ ($a & ($b ^ $c))
        };
    } // Choose, MD5F, SHA1C
    macro_rules! bool3ary_232 {
        ($a:expr, $b:expr, $c:expr) => {
            ($a & $b) ^ ($a & $c) ^ ($b & $c)
        };
    } // Majority, SHA1M

    macro_rules! rotate_state {
        ($s:ident) => {{
            let tmp = $s[7];
            $s[7] = $s[6];
            $s[6] = $s[5];
            $s[5] = $s[4];
            $s[4] = $s[3];
            $s[3] = $s[2];
            $s[2] = $s[1];
            $s[1] = $s[0];
            $s[0] = tmp;
        }};
    }

    let t = x
        .wrapping_add(s[7])
        .wrapping_add(big_sigma1!(s[4]))
        .wrapping_add(bool3ary_202!(s[4], s[5], s[6]));

    s[7] = t
        .wrapping_add(big_sigma0!(s[0]))
        .wrapping_add(bool3ary_232!(s[0], s[1], s[2]));
    s[3] = s[3].wrapping_add(t);

    rotate_state!(s);
}

#[inline(always)]
unsafe fn accumulate_state(dst: &mut State, src: &State) {
    for i in 0..SHA512_HASH_WORDS_NUM {
        dst[i] = dst[i].wrapping_add(src[i]);
    }
}

macro_rules! fn_sha512_update_x {
    ($name:ident, $ty:ident, {
        LOAD = $LOAD:ident,
        ADD64 = $ADD64:ident,
        ALIGNR8 = $ALIGNR8:ident,
        SRL64 = $SRL64:ident,
        SLL64 = $SLL64:ident,
        XOR = $XOR:ident,
    }) => {
        unsafe fn $name(x: &mut [$ty; 8], k64_p: *const $ty) -> $ty {
            // q[2:1]
            let mut t0 = $ALIGNR8::<8>(x[1], x[0]);
            // q[10:9]
            let mut t3 = $ALIGNR8::<8>(x[5], x[4]);
            // q[2:1] >> s0[0]
            let mut t2 = $SRL64::<1>(t0);
            // q[1:0] + q[10:9]
            x[0] = $ADD64(x[0], t3);
            // q[2:1] >> s0[2]
            t3 = $SRL64::<7>(t0);
            // q[2:1] << (64 - s0[1])
            let mut t1 = $SLL64::<{ 64 - 8 }>(t0);
            // (q[2:1] >> s0[2]) ^
            // (q[2:1] >> s0[0])
            t0 = $XOR(t3, t2);
            // q[2:1] >> s0[1]
            t2 = $SRL64::<{ 8 - 1 }>(t2);
            // (q[2:1] >> s0[2]) ^
            // (q[2:1] >> s0[0]) ^
            // q[2:1] << (64 - s0[1])
            t0 = $XOR(t0, t1);
            // q[2:1] << (64 - s0[0])
            t1 = $SLL64::<{ 8 - 1 }>(t1);
            // sigma1(q[2:1])
            t0 = $XOR(t0, t2);
            t0 = $XOR(t0, t1);
            // q[15:14] >> s1[2]
            t3 = $SRL64::<6>(x[7]);
            // q[15:14] >> (64 - s1[1])
            t2 = $SLL64::<{ 64 - 61 }>(x[7]);
            // q[1:0] + sigma0(q[2:1])
            x[0] = $ADD64(x[0], t0);
            // q[15:14] >> s1[0]
            t1 = $SRL64::<19>(x[7]);
            // q[15:14] >> s1[2] ^
            // q[15:14] >> (64 - s1[1])
            t3 = $XOR(t3, t2);
            // q[15:14] >> (64 - s1[0])
            t2 = $SLL64::<{ 61 - 19 }>(t2);
            // q[15:14] >> s1[2] ^
            // q[15:14] >> (64 - s1[1] ^
            // q[15:14] >> s1[0]
            t3 = $XOR(t3, t1);
            // q[15:14] >> s1[1]
            t1 = $SRL64::<{ 61 - 19 }>(t1);
            // sigma1(q[15:14])
            t3 = $XOR(t3, t2);
            t3 = $XOR(t3, t1);

            // q[1:0] + q[10:9] + sigma1(q[15:14]) + sigma0(q[2:1])
            x[0] = $ADD64(x[0], t3);

            // rotate
            let temp = x[0];
            x[0] = x[1];
            x[1] = x[2];
            x[2] = x[3];
            x[3] = x[4];
            x[4] = x[5];
            x[5] = x[6];
            x[6] = x[7];
            x[7] = temp;

            $ADD64(x[7], $LOAD(k64_p))
        }
    };
}

fn_sha512_update_x!(sha512_update_x_avx, __m128i, {
        LOAD = _mm_loadu_si128,
        ADD64 = _mm_add_epi64,
        ALIGNR8 = _mm_alignr_epi8,
        SRL64 = _mm_srli_epi64,
        SLL64 = _mm_slli_epi64,
        XOR = _mm_xor_si128,
});

fn_sha512_update_x!(sha512_update_x_avx2, __m256i, {
        LOAD = _mm256_loadu_si256,
        ADD64 = _mm256_add_epi64,
        ALIGNR8 = _mm256_alignr_epi8,
        SRL64 = _mm256_srli_epi64,
        SLL64 = _mm256_slli_epi64,
        XOR = _mm256_xor_si256,
});

type State = [u64; SHA512_HASH_WORDS_NUM];
type MsgSchedule = [u64; SHA512_BLOCK_WORDS_NUM];
type RoundStates = [u64; SHA512_ROUNDS_NUM];

const SHA512_BLOCK_BYTE_LEN: usize = 128;
const SHA512_ROUNDS_NUM: usize = 80;
const SHA512_HASH_BYTE_LEN: usize = 64;
const SHA512_HASH_WORDS_NUM: usize = SHA512_HASH_BYTE_LEN / size_of::<u64>();
const SHA512_BLOCK_WORDS_NUM: usize = SHA512_BLOCK_BYTE_LEN / size_of::<u64>();
