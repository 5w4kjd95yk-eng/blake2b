#include <metal_stdlib>
using namespace metal;

struct JobParams {
    ulong words[16];
    ulong start_nonce;
    ulong target[4];
    uint input_len;
    uint nonce_offset;
    uint nonce_size;
    uint nonce_little_endian;
    uint hash_little_endian;
    uint max_results;
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

template <uint shift>
inline ulong rotr64(ulong value) {
    return (value >> shift) | (value << (64 - shift));
}

inline ulong byte_swap(ulong value) {
    value = ((value & 0x00ff00ff00ff00ffUL) << 8) |
            ((value >> 8) & 0x00ff00ff00ff00ffUL);
    value = ((value & 0x0000ffff0000ffffUL) << 16) |
            ((value >> 16) & 0x0000ffff0000ffffUL);
    return (value << 32) | (value >> 32);
}

#define G(r, i, a, b, c, d)                         \
    a = a + b + m[sigma[r][2 * i]];                 \
    d = rotr64<32>(d ^ a);                           \
    c = c + d;                                       \
    b = rotr64<24>(b ^ c);                           \
    a = a + b + m[sigma[r][2 * i + 1]];             \
    d = rotr64<16>(d ^ a);                           \
    c = c + d;                                       \
    b = rotr64<63>(b ^ c)

#define ROUND(r)                                     \
    G(r, 0, v[0], v[4], v[8],  v[12]);              \
    G(r, 1, v[1], v[5], v[9],  v[13]);              \
    G(r, 2, v[2], v[6], v[10], v[14]);              \
    G(r, 3, v[3], v[7], v[11], v[15]);              \
    G(r, 4, v[0], v[5], v[10], v[15]);              \
    G(r, 5, v[1], v[6], v[11], v[12]);              \
    G(r, 6, v[2], v[7], v[8],  v[13]);              \
    G(r, 7, v[3], v[4], v[9],  v[14])

kernel void blake2b_mine(
    constant JobParams& job [[buffer(0)]],
    device atomic_uint* result_count [[buffer(1)]],
    device ulong* results [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    const ulong nonce = job.start_nonce + ulong(gid);
    ulong m[16];
    for (uint i = 0; i < 16; ++i) {
        m[i] = job.words[i];
    }

    if (job.nonce_size == 8 && (job.nonce_offset & 7) == 0) {
        m[job.nonce_offset >> 3] = job.nonce_little_endian ? nonce : byte_swap(nonce);
    } else {
        for (uint i = 0; i < job.nonce_size; ++i) {
            const uint source_shift = job.nonce_little_endian
                ? i * 8
                : (job.nonce_size - 1 - i) * 8;
            const ulong byte_value = (nonce >> source_shift) & 0xffUL;
            const uint absolute = job.nonce_offset + i;
            const uint word = absolute >> 3;
            const uint shift = (absolute & 7) * 8;
            m[word] = (m[word] & ~(0xffUL << shift)) | (byte_value << shift);
        }
    }

    ulong v[16] = {
        0x6a09e667f2bdc928UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL, 0x9b05688c2b3e6c1fUL,
        0x1f83d9abfb41bd6bUL, 0x5be0cd19137e2179UL,
        0x6a09e667f3bcc908UL, 0xbb67ae8584caa73bUL,
        0x3c6ef372fe94f82bUL, 0xa54ff53a5f1d36f1UL,
        0x510e527fade682d1UL ^ ulong(job.input_len),
        0x9b05688c2b3e6c1fUL, 0xe07c265404be4294UL,
        0x5be0cd19137e2179UL,
    };

    ROUND(0);
    ROUND(1);
    ROUND(2);
    ROUND(3);
    ROUND(4);
    ROUND(5);
    ROUND(6);
    ROUND(7);
    ROUND(8);
    ROUND(9);
    ROUND(10);
    ROUND(11);

    const ulong h0 = 0x6a09e667f2bdc928UL ^ v[0] ^ v[8];
    const ulong h1 = 0xbb67ae8584caa73bUL ^ v[1] ^ v[9];
    const ulong h2 = 0x3c6ef372fe94f82bUL ^ v[2] ^ v[10];
    const ulong h3 = 0xa54ff53a5f1d36f1UL ^ v[3] ^ v[11];
    ulong hash[4];
    if (job.hash_little_endian) {
        hash[0] = h3;
        hash[1] = h2;
        hash[2] = h1;
        hash[3] = h0;
    } else {
        hash[0] = byte_swap(h0);
        hash[1] = byte_swap(h1);
        hash[2] = byte_swap(h2);
        hash[3] = byte_swap(h3);
    }

    bool accepted = true;
    for (uint i = 0; i < 4; ++i) {
        if (hash[i] < job.target[i]) {
            break;
        }
        if (hash[i] > job.target[i]) {
            accepted = false;
            break;
        }
    }
    if (!accepted) {
        return;
    }

    const uint slot = atomic_fetch_add_explicit(result_count, 1u, memory_order_relaxed);
    if (slot < job.max_results) {
        results[slot] = nonce;
    }
}

#undef ROUND
#undef G
