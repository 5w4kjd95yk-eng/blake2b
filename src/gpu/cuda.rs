#[cfg(blake2b_cuda)]
mod imp {
    use std::{
        ffi::{c_char, c_int, c_void, CStr},
        ptr::NonNull,
    };

    use anyhow::{bail, Context as _, Result};

    use super::super::{Backend, CudaOptions, DeviceInfo, PreparedJob};
    use crate::protocol::JobSpec;

    #[repr(C)]
    struct NativeDeviceInfo {
        name: [c_char; 256],
        compute_major: c_int,
        compute_minor: c_int,
        total_memory: u64,
        usable_memory: u64,
    }

    #[repr(C)]
    struct NativeJobParams {
        words: [u64; 10],
        precomputed_v: [u64; 16],
        start_nonce: u64,
        nonce_count: u64,
        target_prefix: u64,
        generation: u64,
        result_capacity: u32,
        kernel_variant: u32,
        nonces_per_thread: u32,
        block_size: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct NativeResultSummary {
        count: u32,
        overflow: u32,
        generation: u64,
    }

    const _: () = assert!(std::mem::size_of::<NativeJobParams>() == 256);
    const _: () = assert!(std::mem::offset_of!(NativeJobParams, start_nonce) == 208);
    const _: () = assert!(std::mem::offset_of!(NativeJobParams, result_capacity) == 240);
    const _: () = assert!(std::mem::size_of::<NativeResultSummary>() == 16);

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
        fn blake2b_cuda_miner_create(
            context: *mut c_void,
            capacity: u32,
            miner: *mut *mut c_void,
        ) -> c_int;
        fn blake2b_cuda_miner_destroy(miner: *mut c_void) -> c_int;
        fn blake2b_cuda_mine(
            miner: *mut c_void,
            params: *const NativeJobParams,
            summary: *mut NativeResultSummary,
            results: *mut u64,
        ) -> c_int;
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

    const MAX_RESULTS: usize = 256;

    struct Miner {
        raw: NonNull<c_void>,
    }

    unsafe impl Send for Miner {}

    impl Miner {
        fn new(context: &Context) -> Result<Self> {
            let mut raw = std::ptr::null_mut();
            check(unsafe {
                blake2b_cuda_miner_create(context.raw.as_ptr(), MAX_RESULTS as u32, &mut raw)
            })?;
            Ok(Self {
                raw: NonNull::new(raw).expect("successful CUDA miner creation returned null"),
            })
        }
    }

    impl Drop for Miner {
        fn drop(&mut self) {
            let code = unsafe { blake2b_cuda_miner_destroy(self.raw.as_ptr()) };
            if code != 0 {
                eprintln!("failed to destroy CUDA miner: {}", error_message(code));
            }
        }
    }

    struct CudaJob {
        words: [u64; 10],
        precomputed_v: [u64; 16],
        target_prefix: u64,
        generation: u64,
    }

    impl CudaJob {
        fn new(spec: &JobSpec, generation: u64) -> Result<Self> {
            if spec.blob.len() != 80 {
                bail!(
                    "DATUM CUDA backend requires an 80-byte ASIC input, got {} bytes",
                    spec.blob.len()
                );
            }
            let mut words = [0; 10];
            for (word, bytes) in words.iter_mut().zip(spec.blob.chunks_exact(8)) {
                *word = u64::from_le_bytes(bytes.try_into().unwrap());
            }
            Ok(Self {
                precomputed_v: precompute_round_zero(&words),
                words,
                target_prefix: spec.target.words_be()[0],
                generation,
            })
        }
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
        options: CudaOptions,
        miner: Miner,
        _context: Context,
    }

    impl CudaBackend {
        pub fn new(index: usize, batch_size: u32, options: CudaOptions) -> Result<Self> {
            if batch_size == 0 {
                bail!("CUDA batch size must be greater than zero");
            }
            if !matches!(options.nonces_per_thread, 1 | 2 | 4) {
                bail!("CUDA nonces per thread must be 1, 2, or 4");
            }
            if !(32..=1024).contains(&options.block_size) || !options.block_size.is_multiple_of(32)
            {
                bail!("CUDA block size must be a multiple of 32 from 32 through 1024");
            }
            let device = devices()?
                .into_iter()
                .find(|device| device.index == index)
                .ok_or_else(|| anyhow::anyhow!("CUDA device index {index} does not exist"))?;
            let context = Context::new(index)?;
            let miner = Miner::new(&context)?;
            let work_items = u64::from(batch_size).div_ceil(u64::from(options.nonces_per_thread));
            let grid_size = work_items.div_ceil(u64::from(options.block_size));
            eprintln!(
                "CUDA kernel={:?} nonces_per_thread={} block_size={} grid_size={}",
                options.kernel, options.nonces_per_thread, options.block_size, grid_size
            );
            Ok(Self {
                device,
                batch_size: u64::from(batch_size),
                options,
                miner,
                _context: context,
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

        fn prepare_job(&self, spec: &JobSpec, generation: u64) -> Result<Box<dyn PreparedJob>> {
            Ok(Box::new(CudaJob::new(spec, generation)?))
        }

        fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>> {
            let job = job
                .as_any()
                .downcast_ref::<CudaJob>()
                .context("prepared job does not belong to the CUDA backend")?;
            let params = NativeJobParams {
                words: job.words,
                precomputed_v: job.precomputed_v,
                start_nonce,
                nonce_count: self.batch_size,
                target_prefix: job.target_prefix,
                generation: job.generation,
                result_capacity: MAX_RESULTS as u32,
                kernel_variant: match self.options.kernel {
                    crate::config::CudaKernel::Reference => 0,
                    crate::config::CudaKernel::Scalar => 1,
                    crate::config::CudaKernel::ScalarPermute => 2,
                    crate::config::CudaKernel::ScalarPermutePrecompute => 3,
                },
                nonces_per_thread: self.options.nonces_per_thread,
                block_size: self.options.block_size,
            };
            let mut summary = NativeResultSummary::default();
            let mut results = vec![0; MAX_RESULTS];
            check(unsafe {
                blake2b_cuda_mine(
                    self.miner.raw.as_ptr(),
                    &params,
                    &mut summary,
                    results.as_mut_ptr(),
                )
            })?;
            if summary.generation != job.generation {
                bail!(
                    "CUDA result generation mismatch: expected {}, got {}",
                    job.generation,
                    summary.generation
                );
            }
            if summary.overflow != 0 || summary.count as usize > MAX_RESULTS {
                bail!(
                    "CUDA result buffer overflow: {} candidates exceeded capacity {} in one {}-nonce batch",
                    summary.overflow,
                    MAX_RESULTS,
                    self.batch_size
                );
            }
            results.truncate(summary.count as usize);
            Ok(results)
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
            unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned()
        }
    }

    fn precompute_round_zero(words: &[u64; 10]) -> [u64; 16] {
        let mut v = [
            0x6a09_e667_f2bd_c928,
            0xbb67_ae85_84ca_a73b,
            0x3c6e_f372_fe94_f82b,
            0xa54f_f53a_5f1d_36f1,
            0x510e_527f_ade6_82d1,
            0x9b05_688c_2b3e_6c1f,
            0x1f83_d9ab_fb41_bd6b,
            0x5be0_cd19_137e_2179,
            0x6a09_e667_f3bc_c908,
            0xbb67_ae85_84ca_a73b,
            0x3c6e_f372_fe94_f82b,
            0xa54f_f53a_5f1d_36f1,
            0x510e_527f_ade6_82d1 ^ 80,
            0x9b05_688c_2b3e_6c1f,
            0xe07c_2654_04be_4294,
            0x5be0_cd19_137e_2179,
        ];
        precompute_g(&mut v, 0, 4, 8, 12, words[0], words[1]);
        precompute_g(&mut v, 1, 5, 9, 13, words[2], words[3]);
        precompute_g(&mut v, 3, 7, 11, 15, words[6], words[7]);
        v
    }

    fn precompute_g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
        v[d] = (v[d] ^ v[a]).rotate_right(32);
        v[c] = v[c].wrapping_add(v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(24);
        v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
        v[d] = (v[d] ^ v[a]).rotate_right(16);
        v[c] = v[c].wrapping_add(v[d]);
        v[b] = (v[b] ^ v[c]).rotate_right(63);
    }
}

#[cfg(not(blake2b_cuda))]
mod imp {
    use anyhow::{bail, Result};

    use super::super::{Backend, DeviceInfo};

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        bail!("CUDA support is not compiled in; install a CUDA toolkit and rebuild")
    }

    pub fn backend(
        _index: usize,
        _batch_size: u32,
        _options: super::super::CudaOptions,
    ) -> Result<Box<dyn Backend>> {
        bail!("CUDA support is not compiled in; install a CUDA toolkit and rebuild")
    }
}

pub use imp::devices;

pub fn backend(
    index: usize,
    batch_size: u32,
    options: super::CudaOptions,
) -> anyhow::Result<Box<dyn super::Backend>> {
    #[cfg(blake2b_cuda)]
    {
        Ok(Box::new(imp::CudaBackend::new(index, batch_size, options)?))
    }
    #[cfg(not(blake2b_cuda))]
    {
        imp::backend(index, batch_size, options)
    }
}

#[cfg(all(test, blake2b_cuda))]
mod tests {
    use super::imp::{devices, CudaBackend};
    use crate::{
        config::CudaKernel,
        gpu::{Backend, CudaOptions},
        hash::blake2b256,
        protocol::JobSpec,
        target::Target,
    };

    const TEST_NONCES: u32 = 35;

    #[test]
    fn rejects_invalid_launch_options_before_device_access() {
        for options in [
            CudaOptions {
                kernel: CudaKernel::Reference,
                nonces_per_thread: 3,
                block_size: 256,
            },
            CudaOptions {
                kernel: CudaKernel::Reference,
                nonces_per_thread: 1,
                block_size: 48,
            },
        ] {
            assert!(CudaBackend::new(0, TEST_NONCES, options).is_err());
        }
        assert!(CudaBackend::new(0, 0, CudaOptions::default()).is_err());
    }

    #[test]
    fn datum_kernel_matches_rust_at_full_nonce_boundaries() {
        let Ok(available) = devices() else {
            eprintln!("skipping CUDA self-test because the CUDA driver is unavailable");
            return;
        };
        let Some(device) = available.first() else {
            eprintln!("skipping CUDA self-test because no NVIDIA GPU is exposed");
            return;
        };

        for kernel in [
            CudaKernel::Reference,
            CudaKernel::Scalar,
            CudaKernel::ScalarPermute,
            CudaKernel::ScalarPermutePrecompute,
        ] {
            for nonces_per_thread in [1, 2, 4] {
                let options = CudaOptions {
                    kernel,
                    nonces_per_thread,
                    block_size: 32,
                };
                let mut backend = CudaBackend::new(device.index, TEST_NONCES, options).unwrap();
                let starts = [
                    0,
                    u32::MAX as u64 - 15,
                    (1u64 << 32) + 0x1234_5678,
                    u64::MAX - 15,
                ];
                let mut random = 0x6a09_e667_f3bc_c908u64;
                for (case, start_nonce) in starts.into_iter().enumerate() {
                    let mut blob = [0u8; 80];
                    for chunk in blob.chunks_exact_mut(8) {
                        random ^= random << 13;
                        random ^= random >> 7;
                        random ^= random << 17;
                        chunk.copy_from_slice(&random.to_le_bytes());
                    }
                    let hashes = (0..TEST_NONCES)
                        .map(|offset| {
                            let nonce = start_nonce.wrapping_add(u64::from(offset));
                            (nonce, reference_hash(blob, nonce))
                        })
                        .collect::<Vec<_>>();
                    // Selecting an observed prefix explicitly exercises <= equality.
                    let mut prefixes = hashes
                        .iter()
                        .map(|(nonce, hash)| {
                            (*nonce, u64::from_be_bytes(hash[..8].try_into().unwrap()))
                        })
                        .collect::<Vec<_>>();
                    prefixes.sort_unstable_by_key(|(_, prefix)| *prefix);
                    let (equal_nonce, selected_prefix) = prefixes[TEST_NONCES as usize / 2];
                    let target =
                        Target::from_hex(&format!("{selected_prefix:016x}{}", "00".repeat(24)))
                            .unwrap();
                    let spec = job(blob, target);
                    let prepared = backend.prepare_job(&spec, case as u64 + 1).unwrap();
                    let mut actual = backend.mine(prepared.as_ref(), start_nonce).unwrap();
                    let mut expected = hashes
                        .iter()
                        .filter_map(|(nonce, hash)| {
                            let prefix = u64::from_be_bytes(hash[..8].try_into().unwrap());
                            (prefix <= selected_prefix).then_some(*nonce)
                        })
                        .collect::<Vec<_>>();
                    actual.sort_unstable();
                    expected.sort_unstable();
                    assert_eq!(
                        actual, expected,
                        "CUDA mismatch for {kernel:?} x{nonces_per_thread} in boundary case {case}"
                    );
                    assert!(actual.contains(&equal_nonce), "prefix equality was lost");
                    assert!(
                        hashes.iter().any(|(nonce, _)| !actual.contains(nonce)),
                        "self-test target did not select a non-candidate"
                    );
                }
            }
        }
    }

    #[test]
    fn datum_kernel_reports_result_overflow() {
        let Ok(available) = devices() else {
            eprintln!("skipping CUDA overflow test because the CUDA driver is unavailable");
            return;
        };
        let Some(device) = available.first() else {
            eprintln!("skipping CUDA overflow test because no NVIDIA GPU is exposed");
            return;
        };
        let spec = job([0x5a; 80], Target::from_hex(&"ff".repeat(32)).unwrap());
        let mut backend = CudaBackend::new(device.index, 257, Default::default()).unwrap();
        let prepared = backend.prepare_job(&spec, 99).unwrap();
        let error = backend.mine(prepared.as_ref(), 0).unwrap_err();
        assert!(error.to_string().contains("result buffer overflow"));
    }

    fn job(blob: [u8; 80], target: Target) -> JobSpec {
        JobSpec {
            id: "cuda-self-test".to_owned(),
            blob: blob.to_vec(),
            target,
            network_target: None,
            extra_nonce2: "0000000000000000".to_owned(),
            ntime: "0000000000000000".to_owned(),
        }
    }

    fn reference_hash(mut blob: [u8; 80], nonce: u64) -> [u8; 32] {
        blob[32..40].copy_from_slice(&nonce.to_le_bytes());
        blake2b256(&blob)
    }
}
