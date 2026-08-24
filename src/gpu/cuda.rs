#[cfg(feature = "cuda")]
mod imp {
    use std::{
        ffi::{c_char, c_int, c_void, CStr},
        ptr::NonNull,
    };

    use anyhow::{bail, Result};

    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::protocol::JobSpec;

    #[repr(C)]
    struct NativeDeviceInfo {
        name: [c_char; 256],
        compute_major: c_int,
        compute_minor: c_int,
        total_memory: u64,
        usable_memory: u64,
    }

    unsafe extern "C" {
        fn blake2b_cuda_device_count(count: *mut c_int) -> c_int;
        fn blake2b_cuda_device_info(device: c_int, info: *mut NativeDeviceInfo) -> c_int;
        fn blake2b_cuda_context_create(device: c_int, context: *mut *mut c_void) -> c_int;
        fn blake2b_cuda_context_destroy(context: *mut c_void) -> c_int;
        fn blake2b_cuda_buffer_allocate(
            context: *mut c_void,
            size: usize,
            buffer: *mut *mut c_void,
        ) -> c_int;
        fn blake2b_cuda_buffer_release(buffer: *mut c_void) -> c_int;
        fn blake2b_cuda_error_string(error: c_int) -> *const c_char;
    }

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        let mut count = 0;
        check(unsafe { blake2b_cuda_device_count(&mut count) })?;
        (0..count)
            .map(|index| device_info(index as usize))
            .collect()
    }

    fn device_info(index: usize) -> Result<DeviceInfo> {
        let mut native = NativeDeviceInfo {
            name: [0; 256],
            compute_major: 0,
            compute_minor: 0,
            total_memory: 0,
            usable_memory: 0,
        };
        check(unsafe { blake2b_cuda_device_info(index as c_int, &mut native) })?;
        let name = unsafe { CStr::from_ptr(native.name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        Ok(DeviceInfo {
            backend: "CUDA",
            index,
            name,
            compute_capability: Some((native.compute_major as u32, native.compute_minor as u32)),
            total_memory: Some(native.total_memory),
            usable_memory: Some(native.usable_memory),
        })
    }

    struct Context {
        raw: NonNull<c_void>,
    }

    unsafe impl Send for Context {}

    impl Context {
        fn new(device: usize) -> Result<Self> {
            let mut raw = std::ptr::null_mut();
            check(unsafe { blake2b_cuda_context_create(device as c_int, &mut raw) })?;
            Ok(Self {
                raw: NonNull::new(raw).expect("successful CUDA context creation returned null"),
            })
        }

        #[allow(dead_code)]
        fn allocate(&self, size: usize) -> Result<Buffer> {
            let mut raw = std::ptr::null_mut();
            check(unsafe { blake2b_cuda_buffer_allocate(self.raw.as_ptr(), size, &mut raw) })?;
            Ok(Buffer {
                raw: NonNull::new(raw).expect("successful CUDA allocation returned null"),
            })
        }
    }

    impl Drop for Context {
        fn drop(&mut self) {
            let code = unsafe { blake2b_cuda_context_destroy(self.raw.as_ptr()) };
            if code != 0 {
                eprintln!("failed to destroy CUDA context: {}", error_message(code));
            }
        }
    }

    struct Buffer {
        raw: NonNull<c_void>,
    }

    unsafe impl Send for Buffer {}

    impl Drop for Buffer {
        fn drop(&mut self) {
            let code = unsafe { blake2b_cuda_buffer_release(self.raw.as_ptr()) };
            if code != 0 {
                eprintln!("failed to release CUDA buffer: {}", error_message(code));
            }
        }
    }

    pub struct CudaBackend {
        device: DeviceInfo,
        batch_size: u64,
        _context: Context,
    }

    impl CudaBackend {
        pub fn new(index: usize, batch_size: u32) -> Result<Self> {
            let device = devices()?
                .into_iter()
                .find(|device| device.index == index)
                .ok_or_else(|| anyhow::anyhow!("CUDA device index {index} does not exist"))?;
            Ok(Self {
                device,
                batch_size: u64::from(batch_size),
                _context: Context::new(index)?,
            })
        }
    }

    impl Backend for CudaBackend {
        fn device_info(&self) -> &DeviceInfo {
            &self.device
        }

        fn batch_size(&self) -> u64 {
            self.batch_size
        }

        fn prepare_job(&self, _spec: &JobSpec) -> Result<Box<dyn PreparedJob>> {
            bail!("CUDA hashing is not implemented until Sprint 3")
        }

        fn mine(&mut self, _job: &dyn PreparedJob, _start_nonce: u64) -> Result<Vec<u64>> {
            bail!("CUDA hashing is not implemented until Sprint 3")
        }
    }

    fn check(code: c_int) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            bail!("CUDA error {code}: {}", error_message(code))
        }
    }

    fn error_message(code: c_int) -> String {
        let raw = unsafe { blake2b_cuda_error_string(code) };
        if raw.is_null() {
            "unknown CUDA error".to_owned()
        } else {
            unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned()
        }
    }
}

#[cfg(not(feature = "cuda"))]
mod imp {
    use anyhow::{bail, Result};

    use super::super::{Backend, DeviceInfo};

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        bail!("CUDA support is not compiled in; rebuild with --features cuda")
    }

    pub fn backend(_index: usize, _batch_size: u32) -> Result<Box<dyn Backend>> {
        bail!("CUDA support is not compiled in; rebuild with --features cuda")
    }
}

pub use imp::devices;

pub fn backend(index: usize, batch_size: u32) -> anyhow::Result<Box<dyn super::Backend>> {
    #[cfg(feature = "cuda")]
    {
        Ok(Box::new(imp::CudaBackend::new(index, batch_size)?))
    }
    #[cfg(not(feature = "cuda"))]
    {
        imp::backend(index, batch_size)
    }
}
