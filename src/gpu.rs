use anyhow::{bail, Result};

use crate::{config::ByteOrder, protocol::JobSpec};

#[cfg(target_os = "macos")]
const MAX_RESULTS: usize = 256;

#[derive(Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Job {
    words: [u64; 16],
    target: [u64; 4],
    input_len: u32,
    nonce_offset: u32,
    nonce_size: u32,
    nonce_little_endian: bool,
    hash_little_endian: bool,
}

impl Job {
    pub fn new(spec: &JobSpec) -> Result<Self> {
        if spec.blob.len() > 128 {
            bail!(
                "Metal backend supports one Blake2b block, got {} bytes",
                spec.blob.len()
            );
        }
        if !(1..=8).contains(&spec.nonce_size)
            || spec
                .nonce_offset
                .checked_add(spec.nonce_size)
                .is_none_or(|end| end > spec.blob.len())
        {
            bail!("GPU nonce layout does not fit the input blob");
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
            input_len: spec.blob.len() as u32,
            nonce_offset: spec.nonce_offset as u32,
            nonce_size: spec.nonce_size as u32,
            nonce_little_endian: spec.nonce_order == ByteOrder::Little,
            hash_little_endian: spec.hash_order == ByteOrder::Little,
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{mem, ptr};

    use anyhow::{bail, Context, Result};
    use metal::{
        Buffer, CommandQueue, CompileOptions, ComputePipelineState, Device, MTLCommandBufferStatus,
        MTLResourceOptions, MTLSize,
    };
    use objc::rc::autoreleasepool;

    use super::{Job, MAX_RESULTS};

    const SHADER: &str = include_str!("blake2b.metal");

    #[repr(C)]
    struct JobParams {
        words: [u64; 16],
        start_nonce: u64,
        target: [u64; 4],
        input_len: u32,
        nonce_offset: u32,
        nonce_size: u32,
        nonce_little_endian: u32,
        hash_little_endian: u32,
        max_results: u32,
    }

    pub struct Miner {
        device_name: String,
        queue: CommandQueue,
        pipeline: ComputePipelineState,
        job_buffer: Buffer,
        count_buffer: Buffer,
        result_buffer: Buffer,
        batch_size: u32,
    }

    impl Miner {
        pub fn new(batch_size: u32) -> Result<Self> {
            let device = Device::system_default().context("no Metal GPU is available")?;
            let options = CompileOptions::new();
            let library = device
                .new_library_with_source(SHADER, &options)
                .map_err(|error| anyhow::anyhow!("compile Metal Blake2b kernel: {error}"))?;
            let function = library
                .get_function("blake2b_mine", None)
                .map_err(|error| anyhow::anyhow!("load Metal Blake2b kernel: {error}"))?;
            let pipeline = device
                .new_compute_pipeline_state_with_function(&function)
                .map_err(|error| anyhow::anyhow!("create Metal compute pipeline: {error}"))?;
            let shared = MTLResourceOptions::StorageModeShared;
            let job_buffer = device.new_buffer(mem::size_of::<JobParams>() as u64, shared);
            let count_buffer = device.new_buffer(mem::size_of::<u32>() as u64, shared);
            let result_buffer =
                device.new_buffer((MAX_RESULTS * mem::size_of::<u64>()) as u64, shared);
            let device_name = device.name().to_owned();
            let queue = device.new_command_queue();
            Ok(Self {
                device_name,
                queue,
                pipeline,
                job_buffer,
                count_buffer,
                result_buffer,
                batch_size,
            })
        }

        pub fn device_name(&self) -> &str {
            &self.device_name
        }

        pub fn batch_size(&self) -> u32 {
            self.batch_size
        }

        pub fn mine(&mut self, job: &Job, start_nonce: u64) -> Result<Vec<u64>> {
            autoreleasepool(|| self.mine_inner(job, start_nonce))
        }

        fn mine_inner(&mut self, job: &Job, start_nonce: u64) -> Result<Vec<u64>> {
            let params = JobParams {
                words: job.words,
                start_nonce,
                target: job.target,
                input_len: job.input_len,
                nonce_offset: job.nonce_offset,
                nonce_size: job.nonce_size,
                nonce_little_endian: u32::from(job.nonce_little_endian),
                hash_little_endian: u32::from(job.hash_little_endian),
                max_results: MAX_RESULTS as u32,
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
            let execution_width = self.pipeline.thread_execution_width();
            let group_width = self
                .pipeline
                .max_total_threads_per_threadgroup()
                .min(256)
                .max(execution_width);
            encoder.dispatch_threads(
                MTLSize::new(self.batch_size as u64, 1, 1),
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
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use anyhow::{bail, Result};

    use super::Job;

    pub struct Miner;

    impl Miner {
        pub fn new(_batch_size: u32) -> Result<Self> {
            bail!("GPU mining requires macOS and Metal")
        }

        pub fn device_name(&self) -> &str {
            "unavailable"
        }

        pub fn batch_size(&self) -> u32 {
            0
        }

        pub fn mine(&mut self, _job: &Job, _start_nonce: u64) -> Result<Vec<u64>> {
            bail!("GPU mining requires macOS and Metal")
        }
    }
}

pub use imp::Miner;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;
    use crate::{
        config::ByteOrder,
        protocol::{JobSpec, Submit},
        target::Target,
    };

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn metal_matches_reference_for_sia_and_raw_layouts() {
        let Ok(mut miner) = Miner::new(1_024) else {
            eprintln!("skipping Metal test because no GPU is exposed");
            return;
        };

        verify_layout(
            &mut miner,
            vec![0x5a; 80],
            32,
            8,
            ByteOrder::Little,
            ByteOrder::Big,
            10_000,
        );
        verify_layout(
            &mut miner,
            vec![0xa5; 96],
            7,
            4,
            ByteOrder::Big,
            ByteOrder::Little,
            0x0102_0304,
        );
    }

    fn verify_layout(
        miner: &mut Miner,
        blob: Vec<u8>,
        nonce_offset: usize,
        nonce_size: usize,
        nonce_order: ByteOrder,
        hash_order: ByteOrder,
        start_nonce: u64,
    ) {
        let mut hashes = (0..miner.batch_size())
            .map(|offset| {
                let nonce = start_nonce + u64::from(offset);
                (
                    nonce,
                    reference_hash(&blob, nonce_offset, nonce_size, nonce_order, nonce),
                )
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable_by(|(_, left), (_, right)| match hash_order {
            ByteOrder::Big => left.cmp(right),
            ByteOrder::Little => left.iter().rev().cmp(right.iter().rev()),
        });
        let selected = hashes[31].1;
        let target_bytes = match hash_order {
            ByteOrder::Big => selected.to_vec(),
            ByteOrder::Little => selected.iter().rev().copied().collect(),
        };
        let target = Target::from_hex(&hex::encode(target_bytes)).unwrap();
        let spec = JobSpec {
            id: "gpu-test".to_owned(),
            blob,
            target: target.clone(),
            nonce_offset,
            nonce_size,
            nonce_order,
            hash_order,
            submit: Submit::Normal,
        };
        let mut expected = hashes
            .iter()
            .filter_map(|(nonce, hash)| target.accepts(hash, hash_order).then_some(*nonce))
            .collect::<Vec<_>>();
        let mut actual = miner.mine(&Job::new(&spec).unwrap(), start_nonce).unwrap();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    fn reference_hash(
        blob: &[u8],
        nonce_offset: usize,
        nonce_size: usize,
        nonce_order: ByteOrder,
        nonce: u64,
    ) -> [u8; 32] {
        let mut input = blob.to_vec();
        let bytes = match nonce_order {
            ByteOrder::Little => nonce.to_le_bytes(),
            ByteOrder::Big => nonce.to_be_bytes(),
        };
        let source = match nonce_order {
            ByteOrder::Little => &bytes[..nonce_size],
            ByteOrder::Big => &bytes[8 - nonce_size..],
        };
        input[nonce_offset..nonce_offset + nonce_size].copy_from_slice(source);
        ReferenceBlake2b256::digest(input).into()
    }
}
