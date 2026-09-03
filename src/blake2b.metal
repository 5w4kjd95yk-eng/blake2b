#include <metal_stdlib>
using namespace metal;

struct JobParams {
    ulong words[16];
    ulong start_nonce;
    ulong target[4];
    uint max_results;
    uint nonce_count;
};

constant uchar sigma[12][16] = {
    { 0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15},
    {14, 10,  4,  8,  9, 15, 13,  6,  1, 12,  0,  2, 11,  7,  5,  3},
    {11,  8, 12,  0,  5,  2, 15, 13, 10, 14,  3,  6,  7,  1,  9,  4},
    { 7,  9,  3,  1, 13, 12, 11, 14,  2,  6,  5, 10,  4,  0, 15,  8},
    { 9,  0,  5,  7,  2,  4, 10, 15, 14,  1, 11, 12,  6,  8,  3, 13},
    { 2, 12,  6, 10,  0, 11,  8,  3,  4, 13,  7,  5, 15, 14,  1,  9},
    {12,  5,  1, 15, 14, 13,  4, 10,  0,  7,  6,  3,  9,  2,  8, 11},
    {13, 11,  7, 14, 12,  1,  3,  9,  5,  0, 15,  4,  8,  6,  2, 10},
    { 6, 15, 14,  9, 11,  3,  0,  8, 12,  2, 13,  7,  1,  4, 10,  5},
    {10,  2,  8,  4,  7,  6,  1,  5, 15, 11,  9, 14,  3, 12, 13,  0},
    { 0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15},
    {14, 10,  4,  8,  9, 15, 13,  6,  1, 12,  0,  2, 11,  7,  5,  3},
};

inline ulong byte_swap(ulong value) {
    value = ((value & 0x00ff00ff00ff00ffUL) << 8) |
            ((value >> 8) & 0x00ff00ff00ff00ffUL);
    value = ((value & 0x0000ffff0000ffffUL) << 16) |
            ((value >> 16) & 0x0000ffff0000ffffUL);
    return (value << 32) | (value >> 32);
}

template <uint shift>
inline ulong rotr64_split(ulong value) {
    const uint2 halves = as_type<uint2>(value);
    if (shift == 32) {
        return as_type<ulong>(halves.yx);
    }
    if (shift == 24) {
        return as_type<ulong>(uint2(
            (halves.x >> 24) | (halves.y << 8),
            (halves.y >> 24) | (halves.x << 8)));
    }
    if (shift == 16) {
        return as_type<ulong>(uint2(
            (halves.x >> 16) | (halves.y << 16),
            (halves.y >> 16) | (halves.x << 16)));
    }
    return as_type<ulong>(uint2(
        (halves.y >> 31) | (halves.x << 1),
        (halves.x >> 31) | (halves.y << 1)));
}

template <uint shift>
inline ulong rotr64_native(ulong value) {
    return (value >> shift) | (value << (64 - shift));
}

#define G_DATUM(r, i, a, b, c, d)                   \
    a = a + b + m[sigma[r][2 * i]];                 \
    d = rotr64_split<32>(d ^ a);                    \
    c = c + d;                                       \
    b = rotr64_split<24>(b ^ c);                    \
    a = a + b + m[sigma[r][2 * i + 1]];             \
    d = rotr64_split<16>(d ^ a);                    \
    c = c + d;                                       \
    b = rotr64_split<63>(b ^ c)

#define ROUND_DATUM(r)                               \
    G_DATUM(r, 0, v[0], v[4], v[8],  v[12]);        \
    G_DATUM(r, 1, v[1], v[5], v[9],  v[13]);        \
    G_DATUM(r, 2, v[2], v[6], v[10], v[14]);        \
    G_DATUM(r, 3, v[3], v[7], v[11], v[15]);        \
    G_DATUM(r, 4, v[0], v[5], v[10], v[15]);        \
    G_DATUM(r, 5, v[1], v[6], v[11], v[12]);        \
    G_DATUM(r, 6, v[2], v[7], v[8],  v[13]);        \
    G_DATUM(r, 7, v[3], v[4], v[9],  v[14])

inline ulong blake2b_datum_baseline(constant JobParams& job, ulong nonce) {
    ulong m[16] = {
        job.words[0], job.words[1], job.words[2], job.words[3],
        nonce,        job.words[5], job.words[6], job.words[7],
        job.words[8], job.words[9], 0,            0,
        0,            0,            0,            0,
    };
    ulong v[16] = {
        0x6a09e667f2bdc928UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL, 0x9b05688c2b3e6c1fUL,
        0x1f83d9abfb41bd6bUL, 0x5be0cd19137e2179UL,
        0x6a09e667f3bcc908UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL ^ 80UL, 0x9b05688c2b3e6c1fUL,
        0xe07c265404be4294UL, 0x5be0cd19137e2179UL,
    };

    ROUND_DATUM(0);
    ROUND_DATUM(1);
    ROUND_DATUM(2);
    ROUND_DATUM(3);
    ROUND_DATUM(4);
    ROUND_DATUM(5);
    ROUND_DATUM(6);
    ROUND_DATUM(7);
    ROUND_DATUM(8);
    ROUND_DATUM(9);
    ROUND_DATUM(10);
    ROUND_DATUM(11);

    return byte_swap(0x6a09e667f2bdc928UL ^ v[0] ^ v[8]);
}

#define G_SCALAR(NATIVE, a, b, c, d, x, y)          \
    a = a + b + (x);                                 \
    d = NATIVE ? rotr64_native<32>(d ^ a)            \
               : rotr64_split<32>(d ^ a);            \
    c = c + d;                                       \
    b = NATIVE ? rotr64_native<24>(b ^ c)            \
               : rotr64_split<24>(b ^ c);            \
    a = a + b + (y);                                 \
    d = NATIVE ? rotr64_native<16>(d ^ a)            \
               : rotr64_split<16>(d ^ a);            \
    c = c + d;                                       \
    b = NATIVE ? rotr64_native<63>(b ^ c)            \
               : rotr64_split<63>(b ^ c)

#define ROUND_SCALAR(NATIVE,x0,x1,x2,x3,x4,x5,x6,x7,x8,x9,x10,x11,x12,x13,x14,x15) \
    G_SCALAR(NATIVE, v0, v4, v8,  v12, x0,  x1);     \
    G_SCALAR(NATIVE, v1, v5, v9,  v13, x2,  x3);     \
    G_SCALAR(NATIVE, v2, v6, v10, v14, x4,  x5);     \
    G_SCALAR(NATIVE, v3, v7, v11, v15, x6,  x7);     \
    G_SCALAR(NATIVE, v0, v5, v10, v15, x8,  x9);     \
    G_SCALAR(NATIVE, v1, v6, v11, v12, x10, x11);   \
    G_SCALAR(NATIVE, v2, v7, v8,  v13, x12, x13);   \
    G_SCALAR(NATIVE, v3, v4, v9,  v14, x14, x15)

template <bool native_rotation>
inline ulong blake2b_datum_scalar(constant JobParams& job, ulong nonce) {
    const ulong m0 = job.words[0], m1 = job.words[1];
    const ulong m2 = job.words[2], m3 = job.words[3];
    const ulong m4 = nonce, m5 = job.words[5];
    const ulong m6 = job.words[6], m7 = job.words[7];
    const ulong m8 = job.words[8], m9 = job.words[9], z = 0;
    ulong v0 = 0x6a09e667f2bdc928UL, v1 = 0xbb67ae8584caa73bUL;
    ulong v2 = 0x3c6ef372fe94f82bUL, v3 = 0xa54ff53a5f1d36f1UL;
    ulong v4 = 0x510e527fade682d1UL, v5 = 0x9b05688c2b3e6c1fUL;
    ulong v6 = 0x1f83d9abfb41bd6bUL, v7 = 0x5be0cd19137e2179UL;
    ulong v8 = 0x6a09e667f3bcc908UL, v9 = 0xbb67ae8584caa73bUL;
    ulong v10 = 0x3c6ef372fe94f82bUL, v11 = 0xa54ff53a5f1d36f1UL;
    ulong v12 = 0x510e527fade682d1UL ^ 80UL, v13 = 0x9b05688c2b3e6c1fUL;
    ulong v14 = 0xe07c265404be4294UL, v15 = 0x5be0cd19137e2179UL;

    ROUND_SCALAR(native_rotation,m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    ROUND_SCALAR(native_rotation,z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    ROUND_SCALAR(native_rotation,z,m8,z,m0,m5,m2,z,z,z,z,m3,m6,m7,m1,m9,m4);
    ROUND_SCALAR(native_rotation,m7,m9,m3,m1,z,z,z,z,m2,m6,m5,z,m4,m0,z,m8);
    ROUND_SCALAR(native_rotation,m9,m0,m5,m7,m2,m4,z,z,z,m1,z,z,m6,m8,m3,z);
    ROUND_SCALAR(native_rotation,m2,z,m6,z,m0,z,m8,m3,m4,z,m7,m5,z,z,m1,m9);
    ROUND_SCALAR(native_rotation,z,m5,m1,z,z,z,m4,z,m0,m7,m6,m3,m9,m2,m8,z);
    ROUND_SCALAR(native_rotation,z,z,m7,z,z,m1,m3,m9,m5,m0,z,m4,m8,m6,m2,z);
    ROUND_SCALAR(native_rotation,m6,z,z,m9,z,m3,m0,m8,z,m2,z,m7,m1,m4,z,m5);
    ROUND_SCALAR(native_rotation,z,m2,m8,m4,m7,m6,m1,m5,z,z,m9,z,m3,z,z,m0);
    ROUND_SCALAR(native_rotation,m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    ROUND_SCALAR(native_rotation,z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    return byte_swap(0x6a09e667f2bdc928UL ^ v0 ^ v8);
}

inline void emit_result(
    constant JobParams& job,
    ulong nonce,
    device atomic_uint* result_count,
    device ulong* results
) {
    const uint slot = atomic_fetch_add_explicit(result_count, 1u, memory_order_relaxed);
    if (slot < job.max_results) {
        results[slot] = nonce;
    }
}

template <uint variant, uint nonces_per_thread>
inline void blake2b_datum_mine_impl(
    constant JobParams& job,
    device atomic_uint* result_count,
    device ulong* results,
    uint gid
) {
    const ulong first_offset = ulong(gid) * ulong(nonces_per_thread);
    for (uint index = 0; index < nonces_per_thread; ++index) {
        const ulong offset = first_offset + ulong(index);
        if (offset >= ulong(job.nonce_count)) {
            return;
        }
        const ulong nonce = job.start_nonce + offset;
        ulong prefix;
        if (variant == 0) {
            prefix = blake2b_datum_baseline(job, nonce);
        } else if (variant == 1) {
            prefix = blake2b_datum_scalar<false>(job, nonce);
        } else {
            prefix = blake2b_datum_scalar<true>(job, nonce);
        }
        if (prefix <= job.target[0]) {
            emit_result(job, nonce, result_count, results);
        }
    }
}

#define DECLARE_DATUM_KERNEL(NAME, VARIANT, WIDTH)   \
kernel void NAME(                                   \
    constant JobParams& job [[buffer(0)]],           \
    device atomic_uint* result_count [[buffer(1)]],  \
    device ulong* results [[buffer(2)]],             \
    uint gid [[thread_position_in_grid]]) {          \
    blake2b_datum_mine_impl<VARIANT, WIDTH>(         \
        job, result_count, results, gid);             \
}

DECLARE_DATUM_KERNEL(blake2b_datum_baseline_x1, 0, 1)
DECLARE_DATUM_KERNEL(blake2b_datum_baseline_x2, 0, 2)
DECLARE_DATUM_KERNEL(blake2b_datum_baseline_x4, 0, 4)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_split_x1, 1, 1)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_split_x2, 1, 2)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_split_x4, 1, 4)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_native_x1, 2, 1)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_native_x2, 2, 2)
DECLARE_DATUM_KERNEL(blake2b_datum_scalar_native_x4, 2, 4)

#undef DECLARE_DATUM_KERNEL

#undef ROUND_DATUM
#undef G_DATUM
#undef ROUND_SCALAR
#undef G_SCALAR
