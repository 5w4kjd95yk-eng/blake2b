// DATUM's 80-byte, one-block BLAKE2b-256 mining kernel. Build options select
// the conservative array implementation or a scalarized message schedule.

#ifndef KERNEL_VARIANT
#define KERNEL_VARIANT 0
#endif

#ifndef NONCES_PER_ITEM
#define NONCES_PER_ITEM 1
#endif

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

inline ulong rotr64_split(ulong value, uint shift) {
    uint2 halves = as_uint2(value);
    if (shift == 32) return as_ulong(halves.yx);
    if (shift < 32) return as_ulong((uint2)(
        (halves.x >> shift) | (halves.y << (32 - shift)),
        (halves.y >> shift) | (halves.x << (32 - shift))));
    shift -= 32;
    return as_ulong((uint2)(
        (halves.y >> shift) | (halves.x << (32 - shift)),
        (halves.x >> shift) | (halves.y << (32 - shift))));
}

inline ulong byte_swap64(ulong value) {
    return as_ulong(as_uchar8(value).s76543210);
}

#if KERNEL_VARIANT == 2
#define ROTR64(value, shift) rotate((value), (ulong)(64 - (shift)))
#else
#define ROTR64(value, shift) rotr64_split((value), (shift))
#endif

#if KERNEL_VARIANT == 0

#define ARRAY_G(r, i, a, b, c, d)               \
    a = a + b + m[sigma[r][2 * i]];             \
    d = ROTR64(d ^ a, 32);                      \
    c = c + d;                                  \
    b = ROTR64(b ^ c, 24);                      \
    a = a + b + m[sigma[r][2 * i + 1]];         \
    d = ROTR64(d ^ a, 16);                      \
    c = c + d;                                  \
    b = ROTR64(b ^ c, 63)

#define ARRAY_ROUND(r)                           \
    ARRAY_G(r, 0, v[0], v[4], v[8],  v[12]);    \
    ARRAY_G(r, 1, v[1], v[5], v[9],  v[13]);    \
    ARRAY_G(r, 2, v[2], v[6], v[10], v[14]);    \
    ARRAY_G(r, 3, v[3], v[7], v[11], v[15]);    \
    ARRAY_G(r, 4, v[0], v[5], v[10], v[15]);    \
    ARRAY_G(r, 5, v[1], v[6], v[11], v[12]);    \
    ARRAY_G(r, 6, v[2], v[7], v[8],  v[13]);    \
    ARRAY_G(r, 7, v[3], v[4], v[9],  v[14])

inline ulong blake2b_prefix(global const ulong *words, ulong nonce) {
    ulong m[16] = {
        words[0], words[1], words[2], words[3], nonce,
        words[5], words[6], words[7], words[8], words[9],
        0, 0, 0, 0, 0, 0
    };
    ulong v[16] = {
        0x6a09e667f2bdc928UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL, 0x9b05688c2b3e6c1fUL,
        0x1f83d9abfb41bd6bUL, 0x5be0cd19137e2179UL,
        0x6a09e667f3bcc908UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL ^ 80UL, 0x9b05688c2b3e6c1fUL,
        0xe07c265404be4294UL, 0x5be0cd19137e2179UL
    };

    ARRAY_ROUND(0); ARRAY_ROUND(1); ARRAY_ROUND(2); ARRAY_ROUND(3);
    ARRAY_ROUND(4); ARRAY_ROUND(5); ARRAY_ROUND(6); ARRAY_ROUND(7);
    ARRAY_ROUND(8); ARRAY_ROUND(9); ARRAY_ROUND(10); ARRAY_ROUND(11);
    return byte_swap64(0x6a09e667f2bdc928UL ^ v[0] ^ v[8]);
}

#else

#define SCALAR_G(a, b, c, d, x, y)              \
    a = a + b + (x);                            \
    d = ROTR64(d ^ a, 32);                      \
    c = c + d;                                  \
    b = ROTR64(b ^ c, 24);                      \
    a = a + b + (y);                            \
    d = ROTR64(d ^ a, 16);                      \
    c = c + d;                                  \
    b = ROTR64(b ^ c, 63)

#define SCALAR_ROUND(x0,x1,x2,x3,x4,x5,x6,x7,x8,x9,x10,x11,x12,x13,x14,x15) \
    SCALAR_G(v0, v4, v8,  v12, x0,  x1);        \
    SCALAR_G(v1, v5, v9,  v13, x2,  x3);        \
    SCALAR_G(v2, v6, v10, v14, x4,  x5);        \
    SCALAR_G(v3, v7, v11, v15, x6,  x7);        \
    SCALAR_G(v0, v5, v10, v15, x8,  x9);        \
    SCALAR_G(v1, v6, v11, v12, x10, x11);       \
    SCALAR_G(v2, v7, v8,  v13, x12, x13);       \
    SCALAR_G(v3, v4, v9,  v14, x14, x15)

inline ulong blake2b_prefix(global const ulong *words, ulong nonce) {
    const ulong m0 = words[0], m1 = words[1], m2 = words[2], m3 = words[3];
    const ulong m4 = nonce, m5 = words[5], m6 = words[6], m7 = words[7];
    const ulong m8 = words[8], m9 = words[9], z = 0;
    ulong v0 = 0x6a09e667f2bdc928UL, v1 = 0xbb67ae8584caa73bUL;
    ulong v2 = 0x3c6ef372fe94f82bUL, v3 = 0xa54ff53a5f1d36f1UL;
    ulong v4 = 0x510e527fade682d1UL, v5 = 0x9b05688c2b3e6c1fUL;
    ulong v6 = 0x1f83d9abfb41bd6bUL, v7 = 0x5be0cd19137e2179UL;
    ulong v8 = 0x6a09e667f3bcc908UL, v9 = 0xbb67ae8584caa73bUL;
    ulong v10 = 0x3c6ef372fe94f82bUL, v11 = 0xa54ff53a5f1d36f1UL;
    ulong v12 = 0x510e527fade682d1UL ^ 80UL, v13 = 0x9b05688c2b3e6c1fUL;
    ulong v14 = 0xe07c265404be4294UL, v15 = 0x5be0cd19137e2179UL;

    SCALAR_ROUND(m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    SCALAR_ROUND(z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    SCALAR_ROUND(z,m8,z,m0,m5,m2,z,z,z,z,m3,m6,m7,m1,m9,m4);
    SCALAR_ROUND(m7,m9,m3,m1,z,z,z,z,m2,m6,m5,z,m4,m0,z,m8);
    SCALAR_ROUND(m9,m0,m5,m7,m2,m4,z,z,z,m1,z,z,m6,m8,m3,z);
    SCALAR_ROUND(m2,z,m6,z,m0,z,m8,m3,m4,z,m7,m5,z,z,m1,m9);
    SCALAR_ROUND(z,m5,m1,z,z,z,m4,z,m0,m7,m6,m3,m9,m2,m8,z);
    SCALAR_ROUND(z,z,m7,z,z,m1,m3,m9,m5,m0,z,m4,m8,m6,m2,z);
    SCALAR_ROUND(m6,z,z,m9,z,m3,m0,m8,z,m2,z,m7,m1,m4,z,m5);
    SCALAR_ROUND(z,m2,m8,m4,m7,m6,m1,m5,z,z,m9,z,m3,z,z,m0);
    SCALAR_ROUND(m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    SCALAR_ROUND(z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    return byte_swap64(0x6a09e667f2bdc928UL ^ v0 ^ v8);
}

#endif

kernel void blake2b_datum_mine(
    global const ulong *words,
    ulong start_nonce,
    ulong nonce_count,
    ulong target_prefix,
    volatile global uint *counters,
    global ulong *results,
    uint result_capacity)
{
    const ulong first = (ulong)get_global_id(0) * NONCES_PER_ITEM;
    for (uint lane = 0; lane < NONCES_PER_ITEM; ++lane) {
        const ulong offset = first + lane;
        if (offset >= nonce_count) return;
        const ulong nonce = start_nonce + offset;
        if (blake2b_prefix(words, nonce) <= target_prefix) {
            const uint slot = atomic_inc(counters);
            if (slot < result_capacity) results[slot] = nonce;
            else atomic_inc(counters + 1);
        }
    }
}
