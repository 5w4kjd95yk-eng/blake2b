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

    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::protocol::JobSpec;

    const MAX_RESULTS: usize = 256;
    const KERNEL_SOURCE: &str = include_str!("../blake2b.cl");

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
    }

    impl OpenClBackend {
        fn new(index: usize, batch_size: u32) -> Result<Self> {
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
            let program = Program::create_and_build_from_source(&context, KERNEL_SOURCE, "")
                .map_err(|error| anyhow::anyhow!("compile OpenCL Blake2b kernel: {error}"))?;
            let kernel = Kernel::create(&program, "blake2b_datum_mine")
                .context("create OpenCL Blake2b kernel")?;
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
                ExecuteKernel::new(&self.kernel)
                    .set_arg(&self.words)
                    .set_arg(&start_nonce)
                    .set_arg(&self.batch_size)
                    .set_arg(&job.target_prefix)
                    .set_arg(&self.counters)
                    .set_arg(&self.results)
                    .set_arg(&(MAX_RESULTS as u32))
                    .set_global_work_size(self.batch_size as usize)
                    .enqueue_nd_range(&self.queue)?
                    .wait()?;
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

    pub fn backend(index: usize, batch_size: u32) -> Result<Box<dyn Backend>> {
        Ok(Box::new(OpenClBackend::new(index, batch_size)?))
    }
}

#[cfg(not(feature = "opencl"))]
mod imp {
    use super::super::{Backend, DeviceInfo};
    use anyhow::{bail, Result};

    pub fn devices() -> Result<Vec<DeviceInfo>> {
        bail!("OpenCL support is not compiled in; rebuild with --features opencl")
    }

    pub fn backend(_index: usize, _batch_size: u32) -> Result<Box<dyn Backend>> {
        bail!("OpenCL support is not compiled in; rebuild with --features opencl")
    }
}

pub use imp::{backend, devices};

#[cfg(all(test, feature = "opencl"))]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;
    use crate::{protocol::JobSpec, target::Target};

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn opencl_matches_reference_for_datum_layout() {
        let Ok(mut backend) = backend(0, 1_024) else {
            eprintln!("skipping OpenCL self-test because no GPU is exposed");
            return;
        };
        let blob = vec![0x5a; 80];
        let start_nonce = u32::MAX as u64 - 511;
        let mut hashes = (0..backend.batch_size())
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
        let job = backend.prepare_job(&spec, 1).unwrap();
        let mut actual = backend.mine(job.as_ref(), start_nonce).unwrap();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    fn reference_hash(blob: &[u8], nonce: u64) -> [u8; 32] {
        let mut input = blob.to_vec();
        input[32..40].copy_from_slice(&nonce.to_le_bytes());
        ReferenceBlake2b256::digest(input).into()
    }
}
