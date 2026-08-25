#[cfg(feature = "opencl")]
mod imp {
    use std::ptr;

    use anyhow::{bail, Context as _, Result};
    use opencl3::{
        command_queue::CommandQueue,
        context::Context,
        device::{get_all_devices, Device, CL_DEVICE_TYPE_GPU},
        kernel::{ExecuteKernel, Kernel},
        memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_READ_WRITE, CL_MEM_WRITE_ONLY},
        program::Program,
        types::CL_BLOCKING,
    };

    use super::super::OpenClOptions;
    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::config::OpenClKernel;
    use crate::protocol::JobSpec;

    const MAX_RESULTS: usize = 256;
    const KERNEL_SOURCE: &str = include_str!("../blake2b_tuned.cl");

    #[derive(Clone, Copy, Debug)]
    struct KernelSelection {
        variant: OpenClKernel,
        nonces_per_item: u32,
        local_size: Option<usize>,
    }

    impl KernelSelection {
        fn from_options(options: OpenClOptions) -> Self {
            Self {
                variant: options.kernel.unwrap_or(OpenClKernel::Baseline),
                nonces_per_item: options.nonces_per_item.unwrap_or(1),
                local_size: options.local_size,
            }
        }

        fn build_options(self) -> String {
            let variant = match self.variant {
                OpenClKernel::Baseline => 0,
                OpenClKernel::ScalarSplit => 1,
                OpenClKernel::ScalarNative => 2,
            };
            format!(
                "-DKERNEL_VARIANT={variant} -DNONCES_PER_ITEM={}",
                self.nonces_per_item
            )
        }
    }

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        get_all_devices(CL_DEVICE_TYPE_GPU)
            .context("enumerate OpenCL GPU devices")?
            .into_iter()
            .enumerate()
            .map(|(index, id)| device_info(index, Device::new(id)))
            .collect()
    }

    fn device_info(index: usize, device: Device) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            backend: "OpenCL",
            index,
            name: device.name().context("read OpenCL device name")?,
            compute_capability: None,
            total_memory: Some(
                device
                    .global_mem_size()
                    .context("read OpenCL device memory")?,
            ),
            usable_memory: Some(
                device
                    .max_mem_alloc_size()
                    .context("read OpenCL allocation limit")?,
            ),
        })
    }

    struct OpenClJob {
        words: [u64; 16],
        target_prefix: u64,
    }

    impl OpenClJob {
        fn new(spec: &JobSpec) -> Result<Self> {
            if spec.blob.len() != 80 {
                bail!(
                    "DATUM OpenCL backend requires an 80-byte ASIC input, got {} bytes",
                    spec.blob.len()
                );
            }
            let mut block = [0u8; 128];
            block[..80].copy_from_slice(&spec.blob);
            let mut words = [0u64; 16];
            for (word, bytes) in words.iter_mut().zip(block.chunks_exact(8)) {
                *word = u64::from_le_bytes(bytes.try_into().unwrap());
            }
            Ok(Self {
                words,
                target_prefix: spec.target.words_be()[0],
            })
        }
    }

    pub struct OpenClBackend {
        info: DeviceInfo,
        queue: CommandQueue,
        kernel: Kernel,
        words: Buffer<u64>,
        counters: Buffer<u32>,
        results: Buffer<u64>,
        batch_size: u64,
        selection: KernelSelection,
    }

    impl OpenClBackend {
        fn new(index: usize, batch_size: u32, options: OpenClOptions) -> Result<Self> {
            let ids =
                get_all_devices(CL_DEVICE_TYPE_GPU).context("enumerate OpenCL GPU devices")?;
            let id = *ids
                .get(index)
                .with_context(|| format!("OpenCL GPU device index {index} does not exist"))?;
            let device = Device::new(id);
            let info = device_info(index, Device::new(id))?;
            let context = Context::from_device(&device).context("create OpenCL context")?;
            let queue =
                CommandQueue::create_default(&context, 0).context("create OpenCL command queue")?;
            let selection = KernelSelection::from_options(options);
            let build_options = selection.build_options();
            let program =
                Program::create_and_build_from_source(&context, KERNEL_SOURCE, &build_options)
                    .map_err(|error| anyhow::anyhow!("compile OpenCL Blake2b kernel: {error}"))?;
            let kernel = Kernel::create(&program, "blake2b_datum_mine")
                .context("create OpenCL Blake2b kernel")?;
            if let Some(local_size) = selection.local_size {
                let maximum = kernel
                    .get_work_group_size(id)
                    .context("query OpenCL kernel work-group limit")?;
                if local_size > maximum {
                    bail!("OpenCL local size {local_size} exceeds kernel limit {maximum}");
                }
            }
            eprintln!(
                "OpenCL kernel={:?} nonces_per_item={} local_size={}",
                selection.variant,
                selection.nonces_per_item,
                selection
                    .local_size
                    .map_or_else(|| "driver".to_owned(), |size| size.to_string())
            );
            let words = unsafe {
                Buffer::create(&context, CL_MEM_READ_ONLY, 16, ptr::null_mut())
                    .context("allocate OpenCL job buffer")?
            };
            let counters = unsafe {
                Buffer::create(&context, CL_MEM_READ_WRITE, 2, ptr::null_mut())
                    .context("allocate OpenCL counter buffer")?
            };
            let results = unsafe {
                Buffer::create(&context, CL_MEM_WRITE_ONLY, MAX_RESULTS, ptr::null_mut())
                    .context("allocate OpenCL result buffer")?
            };
            Ok(Self {
                info,
                queue,
                kernel,
                words,
                counters,
                results,
                batch_size: u64::from(batch_size),
                selection,
            })
        }
    }

    impl Backend for OpenClBackend {
        fn device_info(&self) -> &DeviceInfo {
            &self.info
        }

        fn batch_size(&self) -> u64 {
            self.batch_size
        }

        fn prepare_job(&self, spec: &JobSpec, _generation: u64) -> Result<Box<dyn PreparedJob>> {
            Ok(Box::new(OpenClJob::new(spec)?))
        }

        fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>> {
            let job = job
                .as_any()
                .downcast_ref::<OpenClJob>()
                .context("prepared job does not belong to the OpenCL backend")?;
            let counters = [0u32; 2];
            unsafe {
                self.queue.enqueue_write_buffer(
                    &mut self.words,
                    CL_BLOCKING,
                    0,
                    &job.words,
                    &[],
                )?;
                self.queue.enqueue_write_buffer(
                    &mut self.counters,
                    CL_BLOCKING,
                    0,
                    &counters,
                    &[],
                )?;
                let work_items = self
                    .batch_size
                    .div_ceil(u64::from(self.selection.nonces_per_item));
                let global_size = if let Some(local_size) = self.selection.local_size {
                    usize::try_from(work_items)?
                        .div_ceil(local_size)
                        .checked_mul(local_size)
                        .context("OpenCL global work size overflow")?
                } else {
                    usize::try_from(work_items)?
                };
                let mut execution = ExecuteKernel::new(&self.kernel);
                execution
                    .set_arg(&self.words)
                    .set_arg(&start_nonce)
                    .set_arg(&self.batch_size)
                    .set_arg(&job.target_prefix)
                    .set_arg(&self.counters)
                    .set_arg(&self.results)
                    .set_arg(&(MAX_RESULTS as u32))
                    .set_global_work_size(global_size);
                if let Some(local_size) = self.selection.local_size {
                    execution.set_local_work_size(local_size);
                }
                execution.enqueue_nd_range(&self.queue)?.wait()?;
            }
            let mut summary = [0u32; 2];
            unsafe {
                self.queue.enqueue_read_buffer(
                    &self.counters,
                    CL_BLOCKING,
                    0,
                    &mut summary,
                    &[],
                )?;
            }
            if summary[1] != 0 || summary[0] as usize > MAX_RESULTS {
                bail!("OpenCL result buffer overflow: {} candidates exceeded capacity {} in one {}-nonce batch", summary[1], MAX_RESULTS, self.batch_size);
            }
            let mut results = vec![0u64; summary[0] as usize];
            if !results.is_empty() {
                unsafe {
                    self.queue.enqueue_read_buffer(
                        &self.results,
                        CL_BLOCKING,
                        0,
                        &mut results,
                        &[],
                    )?;
                }
            }
            Ok(results)
        }
    }

    pub fn backend(
        index: usize,
        batch_size: u32,
        options: OpenClOptions,
    ) -> Result<Box<dyn Backend>> {
        Ok(Box::new(OpenClBackend::new(index, batch_size, options)?))
    }
}

#[cfg(not(feature = "opencl"))]
mod imp {
    use super::super::{Backend, DeviceInfo, OpenClOptions};
    use anyhow::{bail, Result};

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        bail!("OpenCL support is not compiled in; rebuild with --features opencl")
    }

    pub fn backend(
        _index: usize,
        _batch_size: u32,
        _options: OpenClOptions,
    ) -> Result<Box<dyn Backend>> {
        bail!("OpenCL support is not compiled in; rebuild with --features opencl")
    }
}

pub use imp::{backend, devices};

#[cfg(all(test, feature = "opencl"))]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;
    use crate::gpu::OpenClOptions;
    use crate::{
        config::{OpenClKernel, OpenClTuning},
        protocol::JobSpec,
        target::Target,
    };

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn opencl_matches_reference_for_datum_layout() {
        if !devices().is_ok_and(|devices| !devices.is_empty()) {
            eprintln!("skipping OpenCL self-test because no GPU is exposed");
            return;
        }
        let blob = vec![0x5a; 80];
        let start_nonce = u32::MAX as u64 - 511;
        let batch_size = 1_027;
        let mut hashes = (0..u64::from(batch_size))
            .map(|offset| {
                let nonce = start_nonce + offset;
                (nonce, reference_hash(&blob, nonce))
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable_by_key(|(_, hash)| *hash);
        let target = Target::from_hex(&hex::encode(hashes[31].1)).unwrap();
        let spec = JobSpec {
            id: "opencl-test".to_owned(),
            blob,
            target: target.clone(),
            extra_nonce2: "0000000000000000".to_owned(),
            ntime: "0000000000000000".to_owned(),
        };
        let mut expected = hashes
            .iter()
            .filter_map(|(nonce, hash)| target.accepts(hash).then_some(*nonce))
            .collect::<Vec<_>>();
        expected.sort_unstable();
        for variant in [
            OpenClKernel::Baseline,
            OpenClKernel::ScalarSplit,
            OpenClKernel::ScalarNative,
        ] {
            for nonces_per_item in [1, 2, 4] {
                let options = OpenClOptions {
                    tuning: OpenClTuning::Off,
                    kernel: Some(variant),
                    local_size: Some(32),
                    nonces_per_item: Some(nonces_per_item),
                };
                let mut backend = backend(0, batch_size, options).unwrap();
                let job = backend.prepare_job(&spec, 1).unwrap();
                let mut actual = backend.mine(job.as_ref(), start_nonce).unwrap();
                actual.sort_unstable();
                assert_eq!(actual, expected, "{variant:?} x{nonces_per_item}");
            }
        }
    }

    fn reference_hash(blob: &[u8], nonce: u64) -> [u8; 32] {
        let mut input = blob.to_vec();
        input[32..40].copy_from_slice(&nonce.to_le_bytes());
        ReferenceBlake2b256::digest(input).into()
    }
}
