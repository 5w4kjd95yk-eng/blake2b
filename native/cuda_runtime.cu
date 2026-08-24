// nvcc pre-includes cuda_runtime.h. Avoid C++ standard-library headers here:
// bleeding-edge WSL distributions can have glibc math declarations newer than
// the CUDA frontend supports even though the runtime ABI itself is compatible.
extern "C" {

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

const char *blake2b_cuda_error_string(int error) {
    return cudaGetErrorString(static_cast<cudaError_t>(error));
}

}  // extern "C"
