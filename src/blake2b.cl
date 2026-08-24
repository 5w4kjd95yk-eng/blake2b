// DATUM's 80-byte, one-block BLAKE2b-256 mining kernel. The split uint2
// rotations are derived from sgminer-blake2b's tuned Sia OpenCL kernel.

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

#define G(r, i, a, b, c, d)                    \
    a = a + b + m[sigma[r][2 * i]];            \
    d = rotr64_split(d ^ a, 32);                \
    c = c + d;                                  \
    b = rotr64_split(b ^ c, 24);                \
    a = a + b + m[sigma[r][2 * i + 1]];        \
    d = rotr64_split(d ^ a, 16);                \
    c = c + d;                                  \
    b = rotr64_split(b ^ c, 63)

#define ROUND(r)                                \
    G(r, 0, v[0], v[4], v[8],  v[12]);         \
    G(r, 1, v[1], v[5], v[9],  v[13]);         \
    G(r, 2, v[2], v[6], v[10], v[14]);         \
    G(r, 3, v[3], v[7], v[11], v[15]);         \
    G(r, 4, v[0], v[5], v[10], v[15]);         \
    G(r, 5, v[1], v[6], v[11], v[12]);         \
    G(r, 6, v[2], v[7], v[8],  v[13]);         \
    G(r, 7, v[3], v[4], v[9],  v[14])

kernel void blake2b_datum_mine(
    global const ulong *words,
    ulong start_nonce,
    ulong nonce_count,
    ulong target_prefix,
    volatile global uint *counters,
    global ulong *results,
    uint result_capacity)
{
    const ulong offset = get_global_id(0);
    if (offset >= nonce_count) return;
    const ulong nonce = start_nonce + offset;
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

    ROUND(0); ROUND(1); ROUND(2); ROUND(3); ROUND(4); ROUND(5);
    ROUND(6); ROUND(7); ROUND(8); ROUND(9); ROUND(10); ROUND(11);

    const ulong prefix = byte_swap64(0x6a09e667f2bdc928UL ^ v[0] ^ v[8]);
    if (prefix <= target_prefix) {
        const uint slot = atomic_inc(counters);
        if (slot < result_capacity) results[slot] = nonce;
        else atomic_inc(counters + 1);
    }
}
