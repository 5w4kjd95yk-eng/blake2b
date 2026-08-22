const IV: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

const SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

pub fn blake2b256(input: &[u8]) -> [u8; 32] {
    let mut h = IV;
    h[0] ^= 0x0101_0020;
    let mut offset = 0;
    let mut count = 0u128;

    while input.len().saturating_sub(offset) > 128 {
        let block: &[u8; 128] = input[offset..offset + 128].try_into().unwrap();
        count += 128;
        compress(&mut h, block, count, false);
        offset += 128;
    }

    let remaining = &input[offset..];
    let mut final_block = [0u8; 128];
    final_block[..remaining.len()].copy_from_slice(remaining);
    count += remaining.len() as u128;
    compress(&mut h, &final_block, count, true);

    digest(h)
}

#[derive(Clone)]
pub struct PreparedBlock {
    words: [u64; 16],
    len: usize,
    nonce_offset: usize,
    nonce_size: usize,
    nonce_little_endian: bool,
}

impl PreparedBlock {
    pub fn new(
        input: &[u8],
        nonce_offset: usize,
        nonce_size: usize,
        nonce_little_endian: bool,
    ) -> Option<Self> {
        if input.len() > 128 || nonce_size == 0 || nonce_size > 8 {
            return None;
        }
        if nonce_offset.checked_add(nonce_size)? > input.len() {
            return None;
        }
        let mut block = [0u8; 128];
        block[..input.len()].copy_from_slice(input);
        let words = words(&block);
        Some(Self {
            words,
            len: input.len(),
            nonce_offset,
            nonce_size,
            nonce_little_endian,
        })
    }

    pub fn hash4(&self, first_nonce: u64) -> [[u8; 32]; 4] {
        #[cfg(target_arch = "aarch64")]
        if self.nonce_size == 8 && self.nonce_offset.is_multiple_of(8) {
            unsafe {
                return neon::hash4_aligned_nonce(
                    &self.words,
                    self.len,
                    self.nonce_offset / 8,
                    first_nonce,
                    self.nonce_little_endian,
                );
            }
        }

        let mut blocks = [self.words; 4];
        for (lane, block) in blocks.iter_mut().enumerate() {
            self.write_nonce(block, first_nonce.wrapping_add(lane as u64));
        }
        hash4_one_block(&blocks, self.len)
    }

    pub fn datum_candidate_mask(&self, first_nonce: u64, target: [u64; 4]) -> u8 {
        debug_assert_eq!(self.len, 80);
        debug_assert_eq!(self.nonce_offset, 32);
        debug_assert_eq!(self.nonce_size, 8);
        debug_assert!(self.nonce_little_endian);

        #[cfg(target_arch = "aarch64")]
        unsafe {
            neon::datum_candidate_mask(&self.words, first_nonce, target)
        }

        #[cfg(not(target_arch = "aarch64"))]
        {
            let hashes = self.hash4(first_nonce);
            hashes.iter().enumerate().fold(0u8, |mask, (lane, hash)| {
                let mut words = [0u64; 4];
                for (word, bytes) in words.iter_mut().zip(hash.chunks_exact(8)) {
                    *word = u64::from_be_bytes(bytes.try_into().unwrap());
                }
                mask | (u8::from(words <= target) << lane)
            })
        }
    }

    pub fn nonce_hex(&self, nonce: u64) -> String {
        let bytes = if self.nonce_little_endian {
            nonce.to_le_bytes()
        } else {
            nonce.to_be_bytes()
        };
        let range = if self.nonce_little_endian {
            &bytes[..self.nonce_size]
        } else {
            &bytes[8 - self.nonce_size..]
        };
        hex::encode(range)
    }

    fn write_nonce(&self, block: &mut [u64; 16], nonce: u64) {
        let nonce = if self.nonce_little_endian {
            nonce.to_le_bytes()
        } else {
            nonce.to_be_bytes()
        };
        let source = if self.nonce_little_endian {
            &nonce[..self.nonce_size]
        } else {
            &nonce[8 - self.nonce_size..]
        };
        for (index, byte) in source.iter().enumerate() {
            let absolute = self.nonce_offset + index;
            let word = absolute / 8;
            let shift = (absolute % 8) * 8;
            block[word] = (block[word] & !(0xffu64 << shift)) | (u64::from(*byte) << shift);
        }
    }
}

fn hash4_one_block(blocks: &[[u64; 16]; 4], len: usize) -> [[u8; 32]; 4] {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        neon::hash4(blocks, len)
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut output = [[0u8; 32]; 4];
        for lane in 0..4 {
            let mut h = IV;
            h[0] ^= 0x0101_0020;
            let mut block = [0u8; 128];
            for (word, chunk) in blocks[lane].iter().zip(block.chunks_exact_mut(8)) {
                chunk.copy_from_slice(&word.to_le_bytes());
            }
            compress(&mut h, &block, len as u128, true);
            output[lane] = digest(h);
        }
        output
    }
}

fn compress(h: &mut [u64; 8], block: &[u8; 128], count: u128, last: bool) {
    let m = words(block);
    let mut v = [0u64; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= count as u64;
    v[13] ^= (count >> 64) as u64;
    if last {
        v[14] = !v[14];
    }

    for sigma in SIGMA {
        g(&mut v, 0, 4, 8, 12, m[sigma[0]], m[sigma[1]]);
        g(&mut v, 1, 5, 9, 13, m[sigma[2]], m[sigma[3]]);
        g(&mut v, 2, 6, 10, 14, m[sigma[4]], m[sigma[5]]);
        g(&mut v, 3, 7, 11, 15, m[sigma[6]], m[sigma[7]]);
        g(&mut v, 0, 5, 10, 15, m[sigma[8]], m[sigma[9]]);
        g(&mut v, 1, 6, 11, 12, m[sigma[10]], m[sigma[11]]);
        g(&mut v, 2, 7, 8, 13, m[sigma[12]], m[sigma[13]]);
        g(&mut v, 3, 4, 9, 14, m[sigma[14]], m[sigma[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

#[inline(always)]
fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn words(block: &[u8; 128]) -> [u64; 16] {
    let mut words = [0u64; 16];
    for (word, chunk) in words.iter_mut().zip(block.chunks_exact(8)) {
        *word = u64::from_le_bytes(chunk.try_into().unwrap());
    }
    words
}

fn digest(h: [u64; 8]) -> [u8; 32] {
    let mut output = [0u8; 32];
    for (word, chunk) in h[..4].iter().zip(output.chunks_exact_mut(8)) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    output
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    use super::{IV, SIGMA};

    #[derive(Clone, Copy)]
    struct U64x4 {
        lo: uint64x2_t,
        hi: uint64x2_t,
    }

    impl U64x4 {
        #[inline(always)]
        unsafe fn new(a: u64, b: u64, c: u64, d: u64) -> Self {
            Self {
                lo: vld1q_u64([a, b].as_ptr()),
                hi: vld1q_u64([c, d].as_ptr()),
            }
        }

        #[inline(always)]
        unsafe fn splat(value: u64) -> Self {
            Self {
                lo: vdupq_n_u64(value),
                hi: vdupq_n_u64(value),
            }
        }

        #[inline(always)]
        unsafe fn add(self, rhs: Self) -> Self {
            Self {
                lo: vaddq_u64(self.lo, rhs.lo),
                hi: vaddq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn xor(self, rhs: Self) -> Self {
            Self {
                lo: veorq_u64(self.lo, rhs.lo),
                hi: veorq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn and(self, rhs: Self) -> Self {
            Self {
                lo: vandq_u64(self.lo, rhs.lo),
                hi: vandq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn or(self, rhs: Self) -> Self {
            Self {
                lo: vorrq_u64(self.lo, rhs.lo),
                hi: vorrq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn equal(self, rhs: Self) -> Self {
            Self {
                lo: vceqq_u64(self.lo, rhs.lo),
                hi: vceqq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn less_than(self, rhs: Self) -> Self {
            Self {
                lo: vcltq_u64(self.lo, rhs.lo),
                hi: vcltq_u64(self.hi, rhs.hi),
            }
        }

        #[inline(always)]
        unsafe fn swap_bytes(self) -> Self {
            Self {
                lo: vreinterpretq_u64_u8(vrev64q_u8(vreinterpretq_u8_u64(self.lo))),
                hi: vreinterpretq_u64_u8(vrev64q_u8(vreinterpretq_u8_u64(self.hi))),
            }
        }

        #[inline(always)]
        unsafe fn mask(self) -> u8 {
            u8::from(vgetq_lane_u64::<0>(self.lo) != 0)
                | (u8::from(vgetq_lane_u64::<1>(self.lo) != 0) << 1)
                | (u8::from(vgetq_lane_u64::<0>(self.hi) != 0) << 2)
                | (u8::from(vgetq_lane_u64::<1>(self.hi) != 0) << 3)
        }

        #[inline(always)]
        unsafe fn rotr<const N: i32>(self) -> Self {
            match N {
                32 => Self {
                    lo: vreinterpretq_u64_u32(vrev64q_u32(vreinterpretq_u32_u64(self.lo))),
                    hi: vreinterpretq_u64_u32(vrev64q_u32(vreinterpretq_u32_u64(self.hi))),
                },
                24 => Self {
                    lo: vorrq_u64(vshrq_n_u64::<24>(self.lo), vshlq_n_u64::<40>(self.lo)),
                    hi: vorrq_u64(vshrq_n_u64::<24>(self.hi), vshlq_n_u64::<40>(self.hi)),
                },
                16 => Self {
                    lo: vorrq_u64(vshrq_n_u64::<16>(self.lo), vshlq_n_u64::<48>(self.lo)),
                    hi: vorrq_u64(vshrq_n_u64::<16>(self.hi), vshlq_n_u64::<48>(self.hi)),
                },
                63 => Self {
                    lo: vorrq_u64(vshrq_n_u64::<63>(self.lo), vshlq_n_u64::<1>(self.lo)),
                    hi: vorrq_u64(vshrq_n_u64::<63>(self.hi), vshlq_n_u64::<1>(self.hi)),
                },
                _ => unreachable!(),
            }
        }

        #[inline(always)]
        unsafe fn lanes(self) -> [u64; 4] {
            let mut output = [0u64; 4];
            vst1q_u64(output.as_mut_ptr(), self.lo);
            vst1q_u64(output.as_mut_ptr().add(2), self.hi);
            output
        }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn hash4(blocks: &[[u64; 16]; 4], len: usize) -> [[u8; 32]; 4] {
        let mut m = [U64x4::splat(0); 16];
        for i in 0..16 {
            m[i] = U64x4::new(blocks[0][i], blocks[1][i], blocks[2][i], blocks[3][i]);
        }
        compress4(m, len)
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn hash4_aligned_nonce(
        words: &[u64; 16],
        len: usize,
        nonce_word: usize,
        first_nonce: u64,
        little_endian: bool,
    ) -> [[u8; 32]; 4] {
        let mut m = [U64x4::splat(0); 16];
        for i in 0..16 {
            m[i] = U64x4::splat(words[i]);
        }
        let nonce = |lane: u64| {
            let value = first_nonce.wrapping_add(lane);
            if little_endian {
                value
            } else {
                value.swap_bytes()
            }
        };
        m[nonce_word] = U64x4::new(nonce(0), nonce(1), nonce(2), nonce(3));
        compress4(m, len)
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn datum_candidate_mask(
        words: &[u64; 16],
        first_nonce: u64,
        target: [u64; 4],
    ) -> u8 {
        let v = compress4_state_datum(words, first_nonce);
        let mut equal = U64x4::splat(u64::MAX);
        let mut less = U64x4::splat(0);
        for i in 0..4 {
            let initial = if i == 0 { IV[i] ^ 0x0101_0020 } else { IV[i] };
            let digest = U64x4::splat(initial).xor(v[i]).xor(v[i + 8]).swap_bytes();
            let target = U64x4::splat(target[i]);
            less = less.or(equal.and(digest.less_than(target)));
            equal = equal.and(digest.equal(target));
        }
        less.or(equal).mask()
    }

    #[inline(always)]
    unsafe fn compress4_state_datum(words: &[u64; 16], first_nonce: u64) -> [U64x4; 16] {
        let m0 = U64x4::splat(words[0]);
        let m1 = U64x4::splat(words[1]);
        let m2 = U64x4::splat(words[2]);
        let m3 = U64x4::splat(words[3]);
        let m4 = U64x4::new(
            first_nonce,
            first_nonce.wrapping_add(1),
            first_nonce.wrapping_add(2),
            first_nonce.wrapping_add(3),
        );
        let m5 = U64x4::splat(words[5]);
        let m6 = U64x4::splat(words[6]);
        let m7 = U64x4::splat(words[7]);
        let m8 = U64x4::splat(words[8]);
        let m9 = U64x4::splat(words[9]);
        let z = U64x4::splat(0);
        let mut v = [U64x4::splat(0); 16];
        for i in 0..8 {
            let initial = if i == 0 { IV[i] ^ 0x0101_0020 } else { IV[i] };
            v[i] = U64x4::splat(initial);
            v[i + 8] = U64x4::splat(IV[i]);
        }
        v[12] = v[12].xor(U64x4::splat(80));
        v[14] = v[14].xor(U64x4::splat(u64::MAX));

        macro_rules! round4 {
            ($m0:expr, $m1:expr, $m2:expr, $m3:expr,
             $m4:expr, $m5:expr, $m6:expr, $m7:expr,
             $m8:expr, $m9:expr, $m10:expr, $m11:expr,
             $m12:expr, $m13:expr, $m14:expr, $m15:expr) => {{
                g(&mut v, 0, 4, 8, 12, $m0, $m1);
                g(&mut v, 1, 5, 9, 13, $m2, $m3);
                g(&mut v, 2, 6, 10, 14, $m4, $m5);
                g(&mut v, 3, 7, 11, 15, $m6, $m7);
                g(&mut v, 0, 5, 10, 15, $m8, $m9);
                g(&mut v, 1, 6, 11, 12, $m10, $m11);
                g(&mut v, 2, 7, 8, 13, $m12, $m13);
                g(&mut v, 3, 4, 9, 14, $m14, $m15);
            }};
        }

        round4!(m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, z, z, z, z, z, z);
        round4!(z, z, m4, m8, m9, z, z, m6, m1, z, m0, m2, z, m7, m5, m3);
        round4!(z, m8, z, m0, m5, m2, z, z, z, z, m3, m6, m7, m1, m9, m4);
        round4!(m7, m9, m3, m1, z, z, z, z, m2, m6, m5, z, m4, m0, z, m8);
        round4!(m9, m0, m5, m7, m2, m4, z, z, z, m1, z, z, m6, m8, m3, z);
        round4!(m2, z, m6, z, m0, z, m8, m3, m4, z, m7, m5, z, z, m1, m9);
        round4!(z, m5, m1, z, z, z, m4, z, m0, m7, m6, m3, m9, m2, m8, z);
        round4!(z, z, m7, z, z, m1, m3, m9, m5, m0, z, m4, m8, m6, m2, z);
        round4!(m6, z, z, m9, z, m3, m0, m8, z, m2, z, m7, m1, m4, z, m5);
        round4!(z, m2, m8, m4, m7, m6, m1, m5, z, z, m9, z, m3, z, z, m0);
        round4!(m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, z, z, z, z, z, z);
        round4!(z, z, m4, m8, m9, z, z, m6, m1, z, m0, m2, z, m7, m5, m3);

        v
    }

    #[inline(always)]
    unsafe fn compress4(m: [U64x4; 16], len: usize) -> [[u8; 32]; 4] {
        let v = compress4_state(m, len);
        let mut output = [[0u8; 32]; 4];
        for i in 0..4 {
            let initial = if i == 0 { IV[i] ^ 0x0101_0020 } else { IV[i] };
            let lanes = U64x4::splat(initial).xor(v[i]).xor(v[i + 8]).lanes();
            for lane in 0..4 {
                output[lane][i * 8..i * 8 + 8].copy_from_slice(&lanes[lane].to_le_bytes());
            }
        }
        output
    }

    #[inline(always)]
    unsafe fn compress4_state(m: [U64x4; 16], len: usize) -> [U64x4; 16] {
        let mut v = [U64x4::splat(0); 16];
        for i in 0..8 {
            let initial = if i == 0 { IV[i] ^ 0x0101_0020 } else { IV[i] };
            v[i] = U64x4::splat(initial);
            v[i + 8] = U64x4::splat(IV[i]);
        }
        v[12] = v[12].xor(U64x4::splat(len as u64));
        v[14] = v[14].xor(U64x4::splat(u64::MAX));

        for sigma in SIGMA {
            g(&mut v, 0, 4, 8, 12, m[sigma[0]], m[sigma[1]]);
            g(&mut v, 1, 5, 9, 13, m[sigma[2]], m[sigma[3]]);
            g(&mut v, 2, 6, 10, 14, m[sigma[4]], m[sigma[5]]);
            g(&mut v, 3, 7, 11, 15, m[sigma[6]], m[sigma[7]]);
            g(&mut v, 0, 5, 10, 15, m[sigma[8]], m[sigma[9]]);
            g(&mut v, 1, 6, 11, 12, m[sigma[10]], m[sigma[11]]);
            g(&mut v, 2, 7, 8, 13, m[sigma[12]], m[sigma[13]]);
            g(&mut v, 3, 4, 9, 14, m[sigma[14]], m[sigma[15]]);
        }

        v
    }

    #[inline(always)]
    unsafe fn g(v: &mut [U64x4; 16], a: usize, b: usize, c: usize, d: usize, x: U64x4, y: U64x4) {
        v[a] = v[a].add(v[b]).add(x);
        v[d] = v[d].xor(v[a]).rotr::<32>();
        v[c] = v[c].add(v[d]);
        v[b] = v[b].xor(v[c]).rotr::<24>();
        v[a] = v[a].add(v[b]).add(y);
        v[d] = v[d].xor(v[a]).rotr::<16>();
        v[c] = v[c].add(v[d]);
        v[b] = v[b].xor(v[c]).rotr::<63>();
    }
}

#[cfg(test)]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn matches_blake2b_256_vectors() {
        assert_eq!(
            hex::encode(blake2b256(b"")),
            "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
        );
        assert_eq!(
            hex::encode(blake2b256(b"abc")),
            "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319"
        );
    }

    #[test]
    fn four_way_hash_matches_scalar_with_sia_nonce_layout() {
        let header = [0x5au8; 80];
        let prepared = PreparedBlock::new(&header, 32, 8, true).unwrap();
        let hashes = prepared.hash4(42);

        for (lane, hash) in hashes.iter().enumerate() {
            let mut expected_header = header;
            expected_header[32..40].copy_from_slice(&(42 + lane as u64).to_le_bytes());
            assert_eq!(*hash, blake2b256(&expected_header));
        }
    }

    #[test]
    fn datum_candidate_mask_matches_full_digest_comparison() {
        for seed in [0u8, 0x5a, 0xff] {
            let mut header = [0u8; 80];
            for (index, byte) in header.iter_mut().enumerate() {
                *byte = seed.wrapping_add((index as u8).wrapping_mul(17));
            }
            let prepared = PreparedBlock::new(&header, 32, 8, true).unwrap();
            for first_nonce in [42, u64::MAX - 2] {
                let hashes = prepared.hash4(first_nonce);
                for target_hash in hashes {
                    let mut target = [0u64; 4];
                    for (word, bytes) in target.iter_mut().zip(target_hash.chunks_exact(8)) {
                        *word = u64::from_be_bytes(bytes.try_into().unwrap());
                    }
                    let expected = hashes.iter().enumerate().fold(0u8, |mask, (lane, hash)| {
                        let mut words = [0u64; 4];
                        for (word, bytes) in words.iter_mut().zip(hash.chunks_exact(8)) {
                            *word = u64::from_be_bytes(bytes.try_into().unwrap());
                        }
                        mask | (u8::from(words <= target) << lane)
                    });
                    assert_eq!(prepared.datum_candidate_mask(first_nonce, target), expected);
                }
            }
        }
    }

    #[test]
    fn four_way_hash_matches_scalar_for_raw_nonce_layouts() {
        let blob = [0xa5u8; 96];
        for (offset, size, little_endian) in [
            (0, 1, true),
            (3, 4, true),
            (7, 8, true),
            (16, 8, false),
            (55, 4, false),
        ] {
            let prepared = PreparedBlock::new(&blob, offset, size, little_endian).unwrap();
            let hashes = prepared.hash4(0x0102_0304_0506_0708);
            for (lane, hash) in hashes.iter().enumerate() {
                let nonce = 0x0102_0304_0506_0708u64 + lane as u64;
                let nonce_bytes = if little_endian {
                    nonce.to_le_bytes()
                } else {
                    nonce.to_be_bytes()
                };
                let source = if little_endian {
                    &nonce_bytes[..size]
                } else {
                    &nonce_bytes[8 - size..]
                };
                let mut expected = blob;
                expected[offset..offset + size].copy_from_slice(source);
                assert_eq!(*hash, blake2b256(&expected));
            }
        }
    }

    #[test]
    fn hashes_multiple_blocks() {
        let input = [7u8; 256];
        assert_eq!(
            blake2b256(&input).as_slice(),
            ReferenceBlake2b256::digest(input).as_slice()
        );
    }
}
