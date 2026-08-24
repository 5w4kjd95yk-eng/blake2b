#[cfg(target_os = "macos")]
mod imp {
    use std::{mem, ptr};

    use anyhow::{bail, Context, Result};
    use metal::{
        Buffer, CommandQueue, CompileOptions, ComputePipelineState, Device, MTLCommandBufferStatus,
        MTLResourceOptions, MTLSize,
    };
    use objc::rc::autoreleasepool;

    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::protocol::JobSpec;

    const MAX_RESULTS: usize = 256;
    const SHADER: &str = include_str!("../blake2b.metal");

    struct MetalJob {
        words: [u64; 16],
        target: [u64; 4],
    }

    impl MetalJob {
        fn new(spec: &JobSpec) -> Result<Self> {
            if spec.blob.len() != 80 {
                bail!(
                    "DATUM Metal backend requires an 80-byte ASIC input, got {} bytes",
                    spec.blob.len()
                );
            }
            let mut block = [0u8; 128];
            block[..spec.blob.len()].copy_from_slice(&spec.blob);
            let mut words = [0u64; 16];
            for (word, bytes) in words.iter_mut().zip(block.chunks_exact(8)) {
                *word = u64::from_le_bytes(bytes.try_into().unwrap());
            }
            Ok(Self {
                words,
                target: spec.target.words_be(),
            })
        }
    }

    #[repr(C)]
    struct JobParams {
        words: [u64; 16],
        start_nonce: u64,
        target: [u64; 4],
        max_results: u32,
        nonce_count: u32,
    }

    pub struct MetalBackend {
        info: DeviceInfo,
        queue: CommandQueue,
        pipeline: ComputePipelineState,
        job_buffer: Buffer,
        count_buffer: Buffer,
        result_buffer: Buffer,
        batch_size: u32,
    }

    impl MetalBackend {
        pub fn new(batch_size: u32) -> Result<Self> {
            let device = Device::system_default().context("no Metal GPU is available")?;
            let options = CompileOptions::new();
            let library = device
                .new_library_with_source(SHADER, &options)
                .map_err(|error| anyhow::anyhow!("compile Metal Blake2b kernel: {error}"))?;
            let function = library
                .get_function("blake2b_datum_mine", None)
                .map_err(|error| anyhow::anyhow!("load Metal Datum kernel: {error}"))?;
            let pipeline = device
                .new_compute_pipeline_state_with_function(&function)
                .map_err(|error| anyhow::anyhow!("create Metal Datum pipeline: {error}"))?;
            let shared = MTLResourceOptions::StorageModeShared;
            let job_buffer = device.new_buffer(mem::size_of::<JobParams>() as u64, shared);
            let count_buffer = device.new_buffer(mem::size_of::<u32>() as u64, shared);
            let result_buffer =
                device.new_buffer((MAX_RESULTS * mem::size_of::<u64>()) as u64, shared);
            let info = DeviceInfo {
                backend: "Metal",
                index: 0,
                name: device.name().to_owned(),
                compute_capability: None,
                total_memory: None,
                usable_memory: None,
            };
            let queue = device.new_command_queue();
            Ok(Self {
                info,
                queue,
                pipeline,
                job_buffer,
                count_buffer,
                result_buffer,
                batch_size,
            })
        }

        fn mine_inner(&mut self, job: &MetalJob, start_nonce: u64) -> Result<Vec<u64>> {
            let params = JobParams {
                words: job.words,
                start_nonce,
                target: job.target,
                max_results: MAX_RESULTS as u32,
                nonce_count: self.batch_size,
            };
            unsafe {
                ptr::copy_nonoverlapping(
                    &params as *const JobParams as *const u8,
                    self.job_buffer.contents() as *mut u8,
                    mem::size_of::<JobParams>(),
                );
                *(self.count_buffer.contents() as *mut u32) = 0;
            }

            let command_buffer = self.queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&self.pipeline);
            encoder.set_buffer(0, Some(&self.job_buffer), 0);
            encoder.set_buffer(1, Some(&self.count_buffer), 0);
            encoder.set_buffer(2, Some(&self.result_buffer), 0);
            let group_width = 64;
            let thread_count = u64::from(self.batch_size).div_ceil(4);
            encoder.dispatch_threads(
                MTLSize::new(thread_count, 1, 1),
                MTLSize::new(group_width, 1, 1),
            );
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();
            if command_buffer.status() != MTLCommandBufferStatus::Completed {
                bail!(
                    "Metal command buffer ended with status {:?}",
                    command_buffer.status()
                );
            }

            let count = unsafe { *(self.count_buffer.contents() as *const u32) as usize };
            if count > MAX_RESULTS {
                bail!(
                    "Metal result buffer overflow: {count} shares in one {}-nonce batch",
                    self.batch_size
                );
            }
            let results = unsafe {
                std::slice::from_raw_parts(self.result_buffer.contents() as *const u64, count)
            };
            Ok(results.to_vec())
        }
    }

    impl Backend for MetalBackend {
        fn device_info(&self) -> &DeviceInfo {
            &self.info
        }

        fn batch_size(&self) -> u64 {
            u64::from(self.batch_size)
        }

        fn prepare_job(&self, spec: &JobSpec, _generation: u64) -> Result<Box<dyn PreparedJob>> {
            Ok(Box::new(MetalJob::new(spec)?))
        }

        fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>> {
            let job = job
                .as_any()
                .downcast_ref::<MetalJob>()
                .context("prepared job does not belong to the Metal backend")?;
            autoreleasepool(|| self.mine_inner(job, start_nonce))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::{bail, Result};

    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::protocol::JobSpec;

    pub struct MetalBackend {
        info: DeviceInfo,
    }

    impl MetalBackend {
        pub fn new(_batch_size: u32) -> Result<Self> {
            bail!("Metal GPU mining requires macOS")
        }
    }

    impl Backend for MetalBackend {
        fn device_info(&self) -> &DeviceInfo {
            &self.info
        }

        fn batch_size(&self) -> u64 {
            0
        }

        fn prepare_job(&self, _spec: &JobSpec, _generation: u64) -> Result<Box<dyn PreparedJob>> {
            bail!("Metal GPU mining requires macOS")
        }

        fn mine(&mut self, _job: &dyn PreparedJob, _start_nonce: u64) -> Result<Vec<u64>> {
            bail!("Metal GPU mining requires macOS")
        }
    }
}

pub use imp::MetalBackend;
