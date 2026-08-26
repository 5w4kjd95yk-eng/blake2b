// nvcc pre-includes cuda_runtime.h. Avoid C++ standard-library headers here:
// bleeding-edge WSL distributions can have glibc math declarations newer than
// the CUDA frontend supports even though the runtime ABI itself is compatible.
struct Blake2bCudaDeviceInfo {
    char name[256];
    int compute_major;
    int compute_minor;
    unsigned long long total_memory;
    unsigned long long usable_memory;
};

struct Blake2bCudaContext {
    int device;
};

struct Blake2bCudaBuffer {
    int device;
    void *pointer;
    size_t size;
};

struct Blake2bCudaJobParams {
    unsigned long long words[10];
    unsigned long long precomputed_v[16];
    unsigned long long start_nonce;
    unsigned long long nonce_count;
    unsigned long long target_prefix;
    unsigned long long generation;
    unsigned int result_capacity;
    unsigned int kernel_variant;
    unsigned int nonces_per_thread;
    unsigned int block_size;
};

struct Blake2bCudaResultSummary {
    unsigned int count;
    unsigned int overflow;
    unsigned long long generation;
};

struct Blake2bCudaMiner {
    int device;
    unsigned int capacity;
    Blake2bCudaJobParams *params;
    Blake2bCudaResultSummary *summary;
    unsigned long long *results;
};

static_assert(sizeof(Blake2bCudaJobParams) == 256, "unexpected CUDA job layout");
static_assert(__builtin_offsetof(Blake2bCudaJobParams, start_nonce) == 208,
              "unexpected CUDA start_nonce offset");
static_assert(__builtin_offsetof(Blake2bCudaJobParams, result_capacity) == 240,
              "unexpected CUDA result_capacity offset");
static_assert(sizeof(Blake2bCudaResultSummary) == 16, "unexpected CUDA result layout");

__device__ __constant__ unsigned char blake2b_sigma[12][16] = {
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

__device__ __forceinline__ unsigned long long blake2b_rotr(
    unsigned long long value, unsigned int shift) {
    return (value >> shift) | (value << (64 - shift));
}

template <unsigned int shift, bool permute>
__device__ __forceinline__ unsigned long long blake2b_rotr_tuned(
    unsigned long long value) {
    if (!permute || shift == 63) return blake2b_rotr(value, shift);
    union Parts {
        unsigned long long value;
        struct {
            unsigned int low;
            unsigned int high;
        } words;
    } input, output;
    input.value = value;
    if (shift == 32) {
        output.words.low = input.words.high;
        output.words.high = input.words.low;
    } else if (shift == 24) {
        output.words.low = __byte_perm(input.words.high, input.words.low, 0x2107);
        output.words.high = __byte_perm(input.words.high, input.words.low, 0x6543);
    } else {
        output.words.low = __byte_perm(input.words.high, input.words.low, 0x1076);
        output.words.high = __byte_perm(input.words.high, input.words.low, 0x5432);
    }
    return output.value;
}

__device__ __forceinline__ unsigned long long blake2b_bswap(
    unsigned long long value) {
    value = ((value & 0x00ff00ff00ff00ffULL) << 8) |
            ((value >> 8) & 0x00ff00ff00ff00ffULL);
    value = ((value & 0x0000ffff0000ffffULL) << 16) |
            ((value >> 16) & 0x0000ffff0000ffffULL);
    return (value << 32) | (value >> 32);
}

__device__ __forceinline__ void blake2b_g(
    unsigned long long &a, unsigned long long &b,
    unsigned long long &c, unsigned long long &d,
    unsigned long long x, unsigned long long y) {
    a = a + b + x;
    d = blake2b_rotr(d ^ a, 32);
    c += d;
    b = blake2b_rotr(b ^ c, 24);
    a = a + b + y;
    d = blake2b_rotr(d ^ a, 16);
    c += d;
    b = blake2b_rotr(b ^ c, 63);
}

template <bool permute>
__device__ __forceinline__ void blake2b_g_tuned(
    unsigned long long &a, unsigned long long &b,
    unsigned long long &c, unsigned long long &d,
    unsigned long long x, unsigned long long y) {
    a = a + b + x;
    d = blake2b_rotr_tuned<32, permute>(d ^ a);
    c += d;
    b = blake2b_rotr_tuned<24, permute>(b ^ c);
    a = a + b + y;
    d = blake2b_rotr_tuned<16, permute>(d ^ a);
    c += d;
    b = blake2b_rotr_tuned<63, permute>(b ^ c);
}

__device__ __forceinline__ unsigned long long blake2b_datum_prefix_reference(
    const Blake2bCudaJobParams *params, unsigned long long nonce) {
    const unsigned long long iv[8] = {
        0x6a09e667f3bcc908ULL, 0xbb67ae8584caa73bULL,
        0x3c6ef372fe94f82bULL, 0xa54ff53a5f1d36f1ULL,
        0x510e527fade682d1ULL, 0x9b05688c2b3e6c1fULL,
        0x1f83d9abfb41bd6bULL, 0x5be0cd19137e2179ULL,
    };
    unsigned long long m[16] = {};
    for (int index = 0; index < 10; ++index) m[index] = params->words[index];
    // Header bytes 32..40 are the little-endian representation of this word.
    m[4] = nonce;

    unsigned long long h[8];
    for (int index = 0; index < 8; ++index) h[index] = iv[index];
    h[0] ^= 0x01010020ULL;  // fanout 1, depth 1, 32-byte digest, no key
    unsigned long long v[16];
    for (int index = 0; index < 8; ++index) {
        v[index] = h[index];
        v[index + 8] = iv[index];
    }
    v[12] ^= 80ULL;
    v[14] = ~v[14];

    for (int round = 0; round < 12; ++round) {
        const unsigned char *s = blake2b_sigma[round];
        blake2b_g(v[0], v[4], v[8],  v[12], m[s[0]],  m[s[1]]);
        blake2b_g(v[1], v[5], v[9],  v[13], m[s[2]],  m[s[3]]);
        blake2b_g(v[2], v[6], v[10], v[14], m[s[4]],  m[s[5]]);
        blake2b_g(v[3], v[7], v[11], v[15], m[s[6]],  m[s[7]]);
        blake2b_g(v[0], v[5], v[10], v[15], m[s[8]],  m[s[9]]);
        blake2b_g(v[1], v[6], v[11], v[12], m[s[10]], m[s[11]]);
        blake2b_g(v[2], v[7], v[8],  v[13], m[s[12]], m[s[13]]);
        blake2b_g(v[3], v[4], v[9],  v[14], m[s[14]], m[s[15]]);
    }
    return blake2b_bswap(h[0] ^ v[0] ^ v[8]);
}

#define BLAKE2B_SCALAR_G(P, a, b, c, d, x, y) \
    blake2b_g_tuned<P>(a, b, c, d, x, y)

#define BLAKE2B_SCALAR_ROUND(P, x0,x1,x2,x3,x4,x5,x6,x7,x8,x9,x10,x11,x12,x13,x14,x15) \
    BLAKE2B_SCALAR_G(P, v0, v4, v8,  v12, x0,  x1);  \
    BLAKE2B_SCALAR_G(P, v1, v5, v9,  v13, x2,  x3);  \
    BLAKE2B_SCALAR_G(P, v2, v6, v10, v14, x4,  x5);  \
    BLAKE2B_SCALAR_G(P, v3, v7, v11, v15, x6,  x7);  \
    BLAKE2B_SCALAR_G(P, v0, v5, v10, v15, x8,  x9);  \
    BLAKE2B_SCALAR_G(P, v1, v6, v11, v12, x10, x11); \
    BLAKE2B_SCALAR_G(P, v2, v7, v8,  v13, x12, x13); \
    BLAKE2B_SCALAR_G(P, v3, v4, v9,  v14, x14, x15)

template <bool permute, bool precomputed>
__device__ __forceinline__ unsigned long long blake2b_datum_prefix_scalar(
    const Blake2bCudaJobParams *params, unsigned long long nonce) {
    const unsigned long long m0 = params->words[0], m1 = params->words[1];
    const unsigned long long m2 = params->words[2], m3 = params->words[3];
    const unsigned long long m4 = nonce, m5 = params->words[5];
    const unsigned long long m6 = params->words[6], m7 = params->words[7];
    const unsigned long long m8 = params->words[8], m9 = params->words[9];
    const unsigned long long z = 0;
    unsigned long long v0, v1, v2, v3, v4, v5, v6, v7;
    unsigned long long v8, v9, v10, v11, v12, v13, v14, v15;
    if (precomputed) {
        v0 = params->precomputed_v[0]; v1 = params->precomputed_v[1];
        v2 = params->precomputed_v[2]; v3 = params->precomputed_v[3];
        v4 = params->precomputed_v[4]; v5 = params->precomputed_v[5];
        v6 = params->precomputed_v[6]; v7 = params->precomputed_v[7];
        v8 = params->precomputed_v[8]; v9 = params->precomputed_v[9];
        v10 = params->precomputed_v[10]; v11 = params->precomputed_v[11];
        v12 = params->precomputed_v[12]; v13 = params->precomputed_v[13];
        v14 = params->precomputed_v[14]; v15 = params->precomputed_v[15];
        BLAKE2B_SCALAR_G(permute, v2, v6, v10, v14, m4, m5);
        BLAKE2B_SCALAR_G(permute, v0, v5, v10, v15, m8, m9);
        BLAKE2B_SCALAR_G(permute, v1, v6, v11, v12, z, z);
        BLAKE2B_SCALAR_G(permute, v2, v7, v8,  v13, z, z);
        BLAKE2B_SCALAR_G(permute, v3, v4, v9,  v14, z, z);
    } else {
        v0 = 0x6a09e667f2bdc928ULL; v1 = 0xbb67ae8584caa73bULL;
        v2 = 0x3c6ef372fe94f82bULL; v3 = 0xa54ff53a5f1d36f1ULL;
        v4 = 0x510e527fade682d1ULL; v5 = 0x9b05688c2b3e6c1fULL;
        v6 = 0x1f83d9abfb41bd6bULL; v7 = 0x5be0cd19137e2179ULL;
        v8 = 0x6a09e667f3bcc908ULL; v9 = 0xbb67ae8584caa73bULL;
        v10 = 0x3c6ef372fe94f82bULL; v11 = 0xa54ff53a5f1d36f1ULL;
        v12 = 0x510e527fade682d1ULL ^ 80ULL; v13 = 0x9b05688c2b3e6c1fULL;
        v14 = 0xe07c265404be4294ULL; v15 = 0x5be0cd19137e2179ULL;
        BLAKE2B_SCALAR_ROUND(permute,m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    }
    BLAKE2B_SCALAR_ROUND(permute,z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    BLAKE2B_SCALAR_ROUND(permute,z,m8,z,m0,m5,m2,z,z,z,z,m3,m6,m7,m1,m9,m4);
    BLAKE2B_SCALAR_ROUND(permute,m7,m9,m3,m1,z,z,z,z,m2,m6,m5,z,m4,m0,z,m8);
    BLAKE2B_SCALAR_ROUND(permute,m9,m0,m5,m7,m2,m4,z,z,z,m1,z,z,m6,m8,m3,z);
    BLAKE2B_SCALAR_ROUND(permute,m2,z,m6,z,m0,z,m8,m3,m4,z,m7,m5,z,z,m1,m9);
    BLAKE2B_SCALAR_ROUND(permute,z,m5,m1,z,z,z,m4,z,m0,m7,m6,m3,m9,m2,m8,z);
    BLAKE2B_SCALAR_ROUND(permute,z,z,m7,z,z,m1,m3,m9,m5,m0,z,m4,m8,m6,m2,z);
    BLAKE2B_SCALAR_ROUND(permute,m6,z,z,m9,z,m3,m0,m8,z,m2,z,m7,m1,m4,z,m5);
    BLAKE2B_SCALAR_ROUND(permute,z,m2,m8,m4,m7,m6,m1,m5,z,z,m9,z,m3,z,z,m0);
    BLAKE2B_SCALAR_ROUND(permute,m0,m1,m2,m3,m4,m5,m6,m7,m8,m9,z,z,z,z,z,z);
    BLAKE2B_SCALAR_ROUND(permute,z,z,m4,m8,m9,z,z,m6,m1,z,m0,m2,z,m7,m5,m3);
    return blake2b_bswap(0x6a09e667f2bdc928ULL ^ v0 ^ v8);
}

template <unsigned int variant>
__device__ __forceinline__ unsigned long long blake2b_datum_prefix_variant(
    const Blake2bCudaJobParams *params, unsigned long long nonce) {
    if (variant == 0) return blake2b_datum_prefix_reference(params, nonce);
    if (variant == 1) return blake2b_datum_prefix_scalar<false, false>(params, nonce);
    if (variant == 2) return blake2b_datum_prefix_scalar<true, false>(params, nonce);
    return blake2b_datum_prefix_scalar<true, true>(params, nonce);
}

template <unsigned int variant, unsigned int nonces_per_thread>
__global__ void blake2b_datum_kernel(
    const Blake2bCudaJobParams *params, Blake2bCudaResultSummary *summary,
    unsigned long long *results) {
    const unsigned long long first =
        (static_cast<unsigned long long>(blockIdx.x) * blockDim.x + threadIdx.x) *
        nonces_per_thread;
#pragma unroll
    for (unsigned int lane = 0; lane < nonces_per_thread; ++lane) {
        const unsigned long long offset = first + lane;
        if (offset >= params->nonce_count) return;
        const unsigned long long nonce = params->start_nonce + offset;
        if (blake2b_datum_prefix_variant<variant>(params, nonce) <= params->target_prefix) {
            const unsigned int slot = atomicAdd(&summary->count, 1U);
            if (slot < params->result_capacity) {
                results[slot] = nonce;
            } else {
                atomicAdd(&summary->overflow, 1U);
            }
        }
    }
}

#undef BLAKE2B_SCALAR_ROUND
#undef BLAKE2B_SCALAR_G

extern "C" {

int blake2b_cuda_device_count(int *count) {
    if (count == 0) return static_cast<int>(cudaErrorInvalidValue);
    return static_cast<int>(cudaGetDeviceCount(count));
}

int blake2b_cuda_device_info(int device, Blake2bCudaDeviceInfo *info) {
    if (info == 0) return static_cast<int>(cudaErrorInvalidValue);
    cudaDeviceProp properties{};
    cudaError_t error = cudaGetDeviceProperties(&properties, device);
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaSetDevice(device);
    if (error != cudaSuccess) return static_cast<int>(error);
    size_t free_memory = 0;
    size_t total_memory = 0;
    error = cudaMemGetInfo(&free_memory, &total_memory);
    if (error != cudaSuccess) return static_cast<int>(error);
    *info = {};
    for (size_t index = 0; index + 1 < sizeof(info->name) && properties.name[index] != 0;
         ++index)
        info->name[index] = properties.name[index];
    info->compute_major = properties.major;
    info->compute_minor = properties.minor;
    info->total_memory = static_cast<unsigned long long>(total_memory);
    info->usable_memory = static_cast<unsigned long long>(free_memory);
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_context_create(int device, Blake2bCudaContext **context) {
    if (context == 0) return static_cast<int>(cudaErrorInvalidValue);
    *context = 0;
    cudaError_t error = cudaSetDevice(device);
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaFree(0);
    if (error != cudaSuccess) return static_cast<int>(error);
    auto *created = static_cast<Blake2bCudaContext *>(malloc(sizeof(Blake2bCudaContext)));
    if (created == 0) return static_cast<int>(cudaErrorMemoryAllocation);
    created->device = device;
    *context = created;
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_context_destroy(Blake2bCudaContext *context) {
    if (context == 0) return static_cast<int>(cudaSuccess);
    cudaError_t error = cudaSetDevice(context->device);
    free(context);
    return static_cast<int>(error);
}

int blake2b_cuda_buffer_allocate(Blake2bCudaContext *context, size_t size,
                                 Blake2bCudaBuffer **buffer) {
    if (context == 0 || buffer == 0 || size == 0)
        return static_cast<int>(cudaErrorInvalidValue);
    *buffer = 0;
    cudaError_t error = cudaSetDevice(context->device);
    if (error != cudaSuccess) return static_cast<int>(error);
    void *pointer = 0;
    error = cudaMalloc(&pointer, size);
    if (error != cudaSuccess) return static_cast<int>(error);
    auto *created = static_cast<Blake2bCudaBuffer *>(malloc(sizeof(Blake2bCudaBuffer)));
    if (created == 0) {
        cudaFree(pointer);
        return static_cast<int>(cudaErrorMemoryAllocation);
    }
    created->device = context->device;
    created->pointer = pointer;
    created->size = size;
    *buffer = created;
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_buffer_release(Blake2bCudaBuffer *buffer) {
    if (buffer == 0) return static_cast<int>(cudaSuccess);
    cudaError_t error = cudaSetDevice(buffer->device);
    if (error == cudaSuccess) error = cudaFree(buffer->pointer);
    free(buffer);
    return static_cast<int>(error);
}

int blake2b_cuda_miner_create(Blake2bCudaContext *context, unsigned int capacity,
                              Blake2bCudaMiner **miner) {
    if (context == 0 || capacity == 0 || miner == 0)
        return static_cast<int>(cudaErrorInvalidValue);
    *miner = 0;
    cudaError_t error = cudaSetDevice(context->device);
    if (error != cudaSuccess) return static_cast<int>(error);
    auto *created = static_cast<Blake2bCudaMiner *>(malloc(sizeof(Blake2bCudaMiner)));
    if (created == 0) return static_cast<int>(cudaErrorMemoryAllocation);
    *created = {};
    created->device = context->device;
    created->capacity = capacity;
    error = cudaMalloc(&created->params, sizeof(Blake2bCudaJobParams));
    if (error == cudaSuccess)
        error = cudaMalloc(&created->summary, sizeof(Blake2bCudaResultSummary));
    if (error == cudaSuccess)
        error = cudaMalloc(&created->results, capacity * sizeof(unsigned long long));
    if (error != cudaSuccess) {
        cudaFree(created->results);
        cudaFree(created->summary);
        cudaFree(created->params);
        free(created);
        return static_cast<int>(error);
    }
    *miner = created;
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_miner_destroy(Blake2bCudaMiner *miner) {
    if (miner == 0) return static_cast<int>(cudaSuccess);
    cudaError_t error = cudaSetDevice(miner->device);
    if (error == cudaSuccess) error = cudaFree(miner->results);
    cudaError_t next = cudaFree(miner->summary);
    if (error == cudaSuccess) error = next;
    next = cudaFree(miner->params);
    if (error == cudaSuccess) error = next;
    free(miner);
    return static_cast<int>(error);
}

int blake2b_cuda_mine(Blake2bCudaMiner *miner,
                      const Blake2bCudaJobParams *params,
                      Blake2bCudaResultSummary *summary,
                      unsigned long long *results) {
    if (miner == 0 || params == 0 || summary == 0 || results == 0 ||
        params->result_capacity != miner->capacity || params->nonce_count == 0 ||
        params->kernel_variant > 3 ||
        (params->nonces_per_thread != 1 && params->nonces_per_thread != 2 &&
         params->nonces_per_thread != 4) ||
        params->block_size < 32 || params->block_size > 1024 ||
        params->block_size % 32 != 0)
        return static_cast<int>(cudaErrorInvalidValue);
    cudaError_t error = cudaSetDevice(miner->device);
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaMemcpy(miner->params, params, sizeof(*params), cudaMemcpyHostToDevice);
    if (error != cudaSuccess) return static_cast<int>(error);
    // The host owns reset ordering. Never clear this state from inside the kernel.
    error = cudaMemset(miner->summary, 0, sizeof(*summary));
    if (error != cudaSuccess) return static_cast<int>(error);
    Blake2bCudaResultSummary initial{};
    initial.generation = params->generation;
    error = cudaMemcpy(miner->summary, &initial, sizeof(initial), cudaMemcpyHostToDevice);
    if (error != cudaSuccess) return static_cast<int>(error);

    const unsigned long long work_items =
        (params->nonce_count + params->nonces_per_thread - 1) /
        params->nonces_per_thread;
    const unsigned long long blocks =
        (work_items + params->block_size - 1) / params->block_size;
    if (blocks > 0xffffffffULL) return static_cast<int>(cudaErrorInvalidConfiguration);
#define BLAKE2B_LAUNCH(V, W) \
    blake2b_datum_kernel<V, W><<<static_cast<unsigned int>(blocks), params->block_size>>>( \
        miner->params, miner->summary, miner->results)
#define BLAKE2B_LAUNCH_WIDTH(V)                    \
    if (params->nonces_per_thread == 1) {          \
        BLAKE2B_LAUNCH(V, 1);                      \
    } else if (params->nonces_per_thread == 2) {   \
        BLAKE2B_LAUNCH(V, 2);                      \
    } else {                                       \
        BLAKE2B_LAUNCH(V, 4);                      \
    }
    if (params->kernel_variant == 0) {
        BLAKE2B_LAUNCH_WIDTH(0);
    } else if (params->kernel_variant == 1) {
        BLAKE2B_LAUNCH_WIDTH(1);
    } else if (params->kernel_variant == 2) {
        BLAKE2B_LAUNCH_WIDTH(2);
    } else {
        BLAKE2B_LAUNCH_WIDTH(3);
    }
#undef BLAKE2B_LAUNCH_WIDTH
#undef BLAKE2B_LAUNCH
    error = cudaGetLastError();
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaDeviceSynchronize();
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaMemcpy(summary, miner->summary, sizeof(*summary), cudaMemcpyDeviceToHost);
    if (error != cudaSuccess) return static_cast<int>(error);
    const unsigned int copied = summary->count < miner->capacity
        ? summary->count : miner->capacity;
    if (copied != 0)
        error = cudaMemcpy(results, miner->results,
                           copied * sizeof(unsigned long long), cudaMemcpyDeviceToHost);
    return static_cast<int>(error);
}

const char *blake2b_cuda_error_string(int error) {
    return cudaGetErrorString(static_cast<cudaError_t>(error));
}

}  // extern "C"
