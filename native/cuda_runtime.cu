#include <cuda_runtime.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <new>

extern "C" {

struct Blake2bCudaDeviceInfo {
    char name[256];
    int compute_major;
    int compute_minor;
    std::uint64_t total_memory;
    std::uint64_t usable_memory;
};

struct Blake2bCudaContext {
    int device;
};

struct Blake2bCudaBuffer {
    int device;
    void *pointer;
    std::size_t size;
};

int blake2b_cuda_device_count(int *count) {
    if (count == nullptr) return static_cast<int>(cudaErrorInvalidValue);
    return static_cast<int>(cudaGetDeviceCount(count));
}

int blake2b_cuda_device_info(int device, Blake2bCudaDeviceInfo *info) {
    if (info == nullptr) return static_cast<int>(cudaErrorInvalidValue);
    cudaDeviceProp properties{};
    cudaError_t error = cudaGetDeviceProperties(&properties, device);
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaSetDevice(device);
    if (error != cudaSuccess) return static_cast<int>(error);
    std::size_t free_memory = 0;
    std::size_t total_memory = 0;
    error = cudaMemGetInfo(&free_memory, &total_memory);
    if (error != cudaSuccess) return static_cast<int>(error);
    std::memset(info, 0, sizeof(*info));
    std::strncpy(info->name, properties.name, sizeof(info->name) - 1);
    info->compute_major = properties.major;
    info->compute_minor = properties.minor;
    info->total_memory = static_cast<std::uint64_t>(total_memory);
    info->usable_memory = static_cast<std::uint64_t>(free_memory);
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_context_create(int device, Blake2bCudaContext **context) {
    if (context == nullptr) return static_cast<int>(cudaErrorInvalidValue);
    *context = nullptr;
    cudaError_t error = cudaSetDevice(device);
    if (error != cudaSuccess) return static_cast<int>(error);
    error = cudaFree(nullptr);
    if (error != cudaSuccess) return static_cast<int>(error);
    auto *created = new (std::nothrow) Blake2bCudaContext{device};
    if (created == nullptr) return static_cast<int>(cudaErrorMemoryAllocation);
    *context = created;
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_context_destroy(Blake2bCudaContext *context) {
    if (context == nullptr) return static_cast<int>(cudaSuccess);
    cudaError_t error = cudaSetDevice(context->device);
    delete context;
    return static_cast<int>(error);
}

int blake2b_cuda_buffer_allocate(Blake2bCudaContext *context, std::size_t size,
                                 Blake2bCudaBuffer **buffer) {
    if (context == nullptr || buffer == nullptr || size == 0)
        return static_cast<int>(cudaErrorInvalidValue);
    *buffer = nullptr;
    cudaError_t error = cudaSetDevice(context->device);
    if (error != cudaSuccess) return static_cast<int>(error);
    void *pointer = nullptr;
    error = cudaMalloc(&pointer, size);
    if (error != cudaSuccess) return static_cast<int>(error);
    auto *created = new (std::nothrow) Blake2bCudaBuffer{context->device, pointer, size};
    if (created == nullptr) {
        cudaFree(pointer);
        return static_cast<int>(cudaErrorMemoryAllocation);
    }
    *buffer = created;
    return static_cast<int>(cudaSuccess);
}

int blake2b_cuda_buffer_release(Blake2bCudaBuffer *buffer) {
    if (buffer == nullptr) return static_cast<int>(cudaSuccess);
    cudaError_t error = cudaSetDevice(buffer->device);
    if (error == cudaSuccess) error = cudaFree(buffer->pointer);
    delete buffer;
    return static_cast<int>(error);
}

const char *blake2b_cuda_error_string(int error) {
    return cudaGetErrorString(static_cast<cudaError_t>(error));
}

}  // extern "C"
