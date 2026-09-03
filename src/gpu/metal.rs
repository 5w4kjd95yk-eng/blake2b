#[cfg(target_os = "macos")]
mod imp {
    use super::super::{Backend, DeviceInfo, MetalOptions, PreparedJob};
    use crate::{
        config::{MetalKernel, MetalTuning},
        hash,
        protocol::JobSpec,
    };
    use anyhow::{bail, Context, Result};
    use metal::{
        Buffer, CommandQueue, CompileOptions, ComputePipelineDescriptor, ComputePipelineState,
        Device, Library, MTLCommandBufferStatus, MTLResourceOptions, MTLSize,
    };
    use objc::rc::autoreleasepool;
    use serde::{Deserialize, Serialize};
    use std::{
        env,
        ffi::CStr,
        fs, mem,
        path::{Path, PathBuf},
        ptr,
        time::Instant,
    };

    const MAX_RESULTS: usize = 256;
    const SHADER: &str = include_str!("../blake2b.metal");
    const TUNING_REVISION: u32 = 1;
    const TUNING_NONCES: u32 = 1 << 22;
    const FINALIST_NONCES: u32 = 1 << 24;
    const VALIDATION_NONCES: u32 = 1_027;
    const LEGACY_WIDTH: u32 = 4;
    const LEGACY_THREADGROUP_SIZE: usize = 64;

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct KernelSelection {
        variant: MetalKernel,
        nonces_per_thread: u32,
        threadgroup_size: usize,
    }
    #[derive(Default, Deserialize, Serialize)]
    struct TuningCache {
        entries: Vec<TuningCacheEntry>,
    }
    #[derive(Deserialize, Serialize)]
    struct TuningCacheEntry {
        key: String,
        selection: KernelSelection,
    }
    struct SelectedPipeline {
        selection: KernelSelection,
        pipeline: ComputePipelineState,
    }

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
    struct Scratch {
        job: Buffer,
        count: Buffer,
        results: Buffer,
    }
    impl Scratch {
        fn new(device: &Device) -> Self {
            let shared = MTLResourceOptions::StorageModeShared;
            Self {
                job: device.new_buffer(mem::size_of::<JobParams>() as u64, shared),
                count: device.new_buffer(mem::size_of::<u32>() as u64, shared),
                results: device.new_buffer((MAX_RESULTS * mem::size_of::<u64>()) as u64, shared),
            }
        }
        fn write(&self, params: &JobParams) {
            unsafe {
                ptr::copy_nonoverlapping(
                    params as *const JobParams as *const u8,
                    self.job.contents() as *mut u8,
                    mem::size_of::<JobParams>(),
                );
                *(self.count.contents() as *mut u32) = 0;
            }
        }
    }

    fn device_info(device: &Device) -> DeviceInfo {
        DeviceInfo {
            backend: "Metal",
            index: 0,
            name: device.name().to_owned(),
            compute_capability: None,
            total_memory: None,
            usable_memory: None,
        }
    }
    pub fn devices() -> Result<Vec<DeviceInfo>> {
        let device = Device::system_default().context("no Metal GPU is available")?;
        Ok(vec![device_info(&device)])
    }
    fn function_name(selection: KernelSelection) -> String {
        let variant = match selection.variant {
            MetalKernel::Baseline => "baseline",
            MetalKernel::ScalarSplit => "scalar_split",
            MetalKernel::ScalarNative => "scalar_native",
        };
        format!("blake2b_datum_{variant}_x{}", selection.nonces_per_thread)
    }
    fn compile_pipeline(
        device: &Device,
        library: &Library,
        selection: KernelSelection,
    ) -> Result<ComputePipelineState> {
        let name = function_name(selection);
        let function = library
            .get_function(&name, None)
            .map_err(|error| anyhow::anyhow!("load Metal kernel {name}: {error}"))?;
        let descriptor = ComputePipelineDescriptor::new();
        descriptor.set_compute_function(Some(&function));
        descriptor.set_thread_group_size_is_multiple_of_thread_execution_width(true);
        descriptor.set_max_total_threads_per_threadgroup(selection.threadgroup_size as u64);
        device
            .new_compute_pipeline_state(&descriptor)
            .map_err(|error| anyhow::anyhow!("create Metal pipeline {name}: {error}"))
    }
    fn pipeline_limits(
        device: &Device,
        library: &Library,
        variant: MetalKernel,
        nonces_per_thread: u32,
    ) -> Result<(usize, usize)> {
        let selection = KernelSelection {
            variant,
            nonces_per_thread,
            threadgroup_size: LEGACY_THREADGROUP_SIZE,
        };
        let name = function_name(selection);
        let function = library
            .get_function(&name, None)
            .map_err(|error| anyhow::anyhow!("load Metal kernel {name}: {error}"))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|error| anyhow::anyhow!("create Metal pipeline {name}: {error}"))?;
        Ok((
            usize::try_from(pipeline.thread_execution_width())?,
            usize::try_from(pipeline.max_total_threads_per_threadgroup())?,
        ))
    }
    fn validate_selection(
        device: &Device,
        library: &Library,
        selection: KernelSelection,
    ) -> Result<ComputePipelineState> {
        let (width, maximum) = pipeline_limits(
            device,
            library,
            selection.variant,
            selection.nonces_per_thread,
        )?;
        if selection.threadgroup_size > maximum || !selection.threadgroup_size.is_multiple_of(width)
        {
            bail!("Metal threadgroup size {} must be a multiple of execution width {width} and no greater than {maximum}", selection.threadgroup_size);
        }
        compile_pipeline(device, library, selection)
    }
    fn dispatch(
        queue: &CommandQueue,
        pipeline: &ComputePipelineState,
        selection: KernelSelection,
        scratch: &Scratch,
        nonce_count: u32,
    ) -> Result<()> {
        let command_buffer = queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(pipeline);
        encoder.set_buffer(0, Some(&scratch.job), 0);
        encoder.set_buffer(1, Some(&scratch.count), 0);
        encoder.set_buffer(2, Some(&scratch.results), 0);
        let thread_count = u64::from(nonce_count).div_ceil(u64::from(selection.nonces_per_thread));
        encoder.dispatch_threads(
            MTLSize::new(thread_count, 1, 1),
            MTLSize::new(selection.threadgroup_size as u64, 1, 1),
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
        Ok(())
    }

    fn validation_case() -> (JobParams, Vec<u64>) {
        let mut blob = [0x5au8; 80];
        let mut block = [0u8; 128];
        block[..80].copy_from_slice(&blob);
        let mut words = [0u64; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(8)) {
            *word = u64::from_le_bytes(bytes.try_into().unwrap());
        }
        let start_nonce = u64::from(u32::MAX) - 511;
        let mut prefixes = (0..u64::from(VALIDATION_NONCES))
            .map(|offset| {
                let nonce = start_nonce.wrapping_add(offset);
                blob[32..40].copy_from_slice(&nonce.to_le_bytes());
                let digest = hash::blake2b256(&blob);
                (nonce, u64::from_be_bytes(digest[..8].try_into().unwrap()))
            })
            .collect::<Vec<_>>();
        prefixes.sort_unstable_by_key(|(_, prefix)| *prefix);
        let target_prefix = prefixes[31].1;
        let expected = prefixes
            .into_iter()
            .filter_map(|(nonce, prefix)| (prefix <= target_prefix).then_some(nonce))
            .collect();
        (
            JobParams {
                words,
                start_nonce,
                target: [target_prefix, 0, 0, 0],
                max_results: MAX_RESULTS as u32,
                nonce_count: VALIDATION_NONCES,
            },
            expected,
        )
    }
    fn validate_candidate(
        queue: &CommandQueue,
        pipeline: &ComputePipelineState,
        selection: KernelSelection,
        scratch: &Scratch,
    ) -> Result<()> {
        let (params, mut expected) = validation_case();
        scratch.write(&params);
        dispatch(queue, pipeline, selection, scratch, VALIDATION_NONCES)?;
        let count = unsafe { *(scratch.count.contents() as *const u32) as usize };
        if count > MAX_RESULTS {
            bail!("Metal tuning validation result buffer overflow");
        }
        let mut actual = unsafe {
            std::slice::from_raw_parts(scratch.results.contents() as *const u64, count).to_vec()
        };
        actual.sort_unstable();
        expected.sort_unstable();
        if actual != expected {
            bail!("Metal tuning candidate failed DATUM validation");
        }
        Ok(())
    }
    fn measure_candidate(
        queue: &CommandQueue,
        pipeline: &ComputePipelineState,
        selection: KernelSelection,
        scratch: &Scratch,
        nonce_count: u32,
    ) -> Result<u64> {
        let mut samples = Vec::with_capacity(5);
        for iteration in 0u64..7 {
            scratch.write(&JobParams {
                words: [0x5a5a_5a5a_5a5a_5a5a; 16],
                start_nonce: iteration * u64::from(nonce_count),
                target: [0; 4],
                max_results: MAX_RESULTS as u32,
                nonce_count,
            });
            let start = Instant::now();
            dispatch(queue, pipeline, selection, scratch, nonce_count)?;
            if iteration >= 2 {
                samples.push(u64::try_from(start.elapsed().as_nanos())?);
            }
        }
        samples.sort_unstable();
        Ok(samples[2])
    }

    fn selection_matches_options(selection: KernelSelection, options: MetalOptions) -> bool {
        options
            .kernel
            .is_none_or(|value| value == selection.variant)
            && options
                .nonces_per_thread
                .is_none_or(|value| value == selection.nonces_per_thread)
            && options
                .threadgroup_size
                .is_none_or(|value| value == selection.threadgroup_size)
    }
    fn source_fingerprint() -> u64 {
        SHADER.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }
    fn os_release() -> String {
        let mut value = mem::MaybeUninit::<libc::utsname>::zeroed();
        unsafe {
            if libc::uname(value.as_mut_ptr()) == 0 {
                return CStr::from_ptr(value.assume_init_ref().release.as_ptr())
                    .to_string_lossy()
                    .into_owned();
            }
        }
        "unknown".to_owned()
    }
    fn tuning_key(device: &Device, options: MetalOptions) -> String {
        format!("revision={TUNING_REVISION};source={:016x};device={};registry={};os={};requested_kernel={:?};requested_width={:?};requested_threadgroup={:?}",
            source_fingerprint(), device.name(), device.registry_id(), os_release(), options.kernel,
            options.nonces_per_thread, options.threadgroup_size)
    }
    fn cache_path() -> Option<PathBuf> {
        Some(
            PathBuf::from(env::var_os("HOME")?)
                .join("Library/Caches/blake2b-miner/metal-tuning.json"),
        )
    }
    fn read_cached_selection(path: &Path, key: &str) -> Option<KernelSelection> {
        let cache: TuningCache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        cache
            .entries
            .into_iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.selection)
    }
    fn write_cached_selection(path: &Path, key: String, selection: KernelSelection) -> Result<()> {
        let mut cache = fs::read(path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<TuningCache>(&contents).ok())
            .unwrap_or_default();
        cache.entries.retain(|entry| entry.key != key);
        cache.entries.push(TuningCacheEntry { key, selection });
        let parent = path.parent().context("Metal tuning cache has no parent")?;
        fs::create_dir_all(parent).context("create Metal tuning cache directory")?;
        let temporary = parent.join(format!(".metal-tuning-{}.tmp", std::process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(&cache)?)
            .context("write temporary Metal tuning cache")?;
        fs::rename(&temporary, path).context("replace Metal tuning cache")?;
        Ok(())
    }
    fn threadgroup_candidates(
        execution_width: usize,
        maximum: usize,
        requested: Option<usize>,
    ) -> Result<Vec<usize>> {
        if let Some(size) = requested {
            if size > maximum || !size.is_multiple_of(execution_width) {
                bail!("Metal threadgroup size {size} must be a multiple of execution width {execution_width} and no greater than {maximum}");
            }
            return Ok(vec![size]);
        }
        let mut candidates = Vec::new();
        let mut size = execution_width;
        while size <= maximum {
            candidates.push(size);
            let Some(next) = size.checked_mul(2) else {
                break;
            };
            size = next;
        }
        Ok(candidates)
    }
    fn legacy_selection(options: MetalOptions) -> KernelSelection {
        KernelSelection {
            variant: options.kernel.unwrap_or(MetalKernel::Baseline),
            nonces_per_thread: options.nonces_per_thread.unwrap_or(LEGACY_WIDTH),
            threadgroup_size: options.threadgroup_size.unwrap_or(LEGACY_THREADGROUP_SIZE),
        }
    }

    fn choose_pipeline(
        device: &Device,
        library: &Library,
        queue: &CommandQueue,
        scratch: &Scratch,
        options: MetalOptions,
    ) -> Result<SelectedPipeline> {
        if options.tuning == MetalTuning::Off {
            let selection = legacy_selection(options);
            let pipeline = validate_selection(device, library, selection)?;
            validate_candidate(queue, &pipeline, selection, scratch)?;
            return Ok(SelectedPipeline {
                selection,
                pipeline,
            });
        }
        let key = tuning_key(device, options);
        let path = cache_path();
        if options.tuning == MetalTuning::Auto {
            if let Some(selection) = path
                .as_deref()
                .and_then(|path| read_cached_selection(path, &key))
                .filter(|selection| selection_matches_options(*selection, options))
            {
                if let Ok(pipeline) = validate_selection(device, library, selection) {
                    if validate_candidate(queue, &pipeline, selection, scratch).is_ok() {
                        eprintln!("Metal tuning cache hit");
                        return Ok(SelectedPipeline {
                            selection,
                            pipeline,
                        });
                    }
                }
                eprintln!("Metal cached tuning selection is unusable; retuning");
            }
        }
        let variants = options.kernel.map_or_else(
            || {
                vec![
                    MetalKernel::Baseline,
                    MetalKernel::ScalarSplit,
                    MetalKernel::ScalarNative,
                ]
            },
            |variant| vec![variant],
        );
        let widths = options
            .nonces_per_thread
            .map_or_else(|| vec![1, 2, 4], |width| vec![width]);
        let mut candidates = Vec::new();
        for variant in variants {
            for nonces_per_thread in widths.iter().copied() {
                let (execution_width, maximum) =
                    pipeline_limits(device, library, variant, nonces_per_thread)?;
                for threadgroup_size in
                    threadgroup_candidates(execution_width, maximum, options.threadgroup_size)?
                {
                    let selection = KernelSelection {
                        variant,
                        nonces_per_thread,
                        threadgroup_size,
                    };
                    let result =
                        compile_pipeline(device, library, selection).and_then(|pipeline| {
                            validate_candidate(queue, &pipeline, selection, scratch)?;
                            let elapsed = measure_candidate(
                                queue,
                                &pipeline,
                                selection,
                                scratch,
                                TUNING_NONCES,
                            )?;
                            Ok((elapsed, selection, pipeline))
                        });
                    match result {
                        Ok((elapsed, selection, pipeline)) => {
                            eprintln!("Metal tuning candidate kernel={:?} nonces_per_thread={} threadgroup={} {:.3} MH/s",
                                selection.variant, selection.nonces_per_thread, selection.threadgroup_size,
                                f64::from(TUNING_NONCES) / elapsed as f64 * 1_000.0);
                            candidates.push((elapsed, selection, pipeline));
                        }
                        Err(error) => eprintln!("Metal tuning rejected {variant:?} x{nonces_per_thread} threadgroup={threadgroup_size}: {error:#}"),
                    }
                }
            }
        }
        candidates.sort_by_key(|(elapsed, _, _)| *elapsed);
        candidates.truncate(3);
        if candidates.is_empty() {
            bail!("no valid Metal tuning candidate");
        }
        let mut finalists = Vec::new();
        for (_, selection, pipeline) in candidates {
            let elapsed = measure_candidate(queue, &pipeline, selection, scratch, FINALIST_NONCES)?;
            eprintln!(
                "Metal tuning finalist kernel={:?} nonces_per_thread={} threadgroup={} {:.3} MH/s",
                selection.variant,
                selection.nonces_per_thread,
                selection.threadgroup_size,
                f64::from(FINALIST_NONCES) / elapsed as f64 * 1_000.0
            );
            finalists.push((elapsed, selection, pipeline));
        }
        finalists.sort_by_key(|(elapsed, _, _)| *elapsed);
        let (_, mut selection, mut pipeline) = finalists.remove(0);
        if options.kernel.is_none()
            && options.nonces_per_thread.is_none()
            && options.threadgroup_size.is_none()
        {
            let legacy = legacy_selection(options);
            let legacy_pipeline = compile_pipeline(device, library, legacy)?;
            let legacy_elapsed =
                measure_candidate(queue, &legacy_pipeline, legacy, scratch, FINALIST_NONCES)?;
            let winner_elapsed =
                measure_candidate(queue, &pipeline, selection, scratch, FINALIST_NONCES)?;
            if winner_elapsed.saturating_mul(100) >= legacy_elapsed.saturating_mul(99) {
                selection = legacy;
                pipeline = legacy_pipeline;
                eprintln!("Metal tuning retained legacy selection (gain below 1%)");
            }
        }
        if let Some(path) = path {
            if let Err(error) = write_cached_selection(&path, key, selection) {
                eprintln!("Metal tuning cache was not written: {error:#}");
            }
        }
        Ok(SelectedPipeline {
            selection,
            pipeline,
        })
    }

    pub struct MetalBackend {
        info: DeviceInfo,
        queue: CommandQueue,
        pipeline: ComputePipelineState,
        scratch: Scratch,
        batch_size: u32,
        selection: KernelSelection,
    }
    impl MetalBackend {
        pub fn new(batch_size: u32, options: MetalOptions) -> Result<Self> {
            let device = Device::system_default().context("no Metal GPU is available")?;
            let library = device
                .new_library_with_source(SHADER, &CompileOptions::new())
                .map_err(|error| anyhow::anyhow!("compile Metal Blake2b kernels: {error}"))?;
            let queue = device.new_command_queue();
            let scratch = Scratch::new(&device);
            let selected = choose_pipeline(&device, &library, &queue, &scratch, options)?;
            let execution_width = selected.pipeline.thread_execution_width();
            eprintln!("Metal kernel={:?} nonces_per_thread={} threadgroup_size={} execution_width={execution_width}",
                selected.selection.variant, selected.selection.nonces_per_thread,
                selected.selection.threadgroup_size);
            Ok(Self {
                info: device_info(&device),
                queue,
                pipeline: selected.pipeline,
                scratch,
                batch_size,
                selection: selected.selection,
            })
        }
        fn mine_inner(&mut self, job: &MetalJob, start_nonce: u64) -> Result<Vec<u64>> {
            self.scratch.write(&JobParams {
                words: job.words,
                start_nonce,
                target: job.target,
                max_results: MAX_RESULTS as u32,
                nonce_count: self.batch_size,
            });
            dispatch(
                &self.queue,
                &self.pipeline,
                self.selection,
                &self.scratch,
                self.batch_size,
            )?;
            let count = unsafe { *(self.scratch.count.contents() as *const u32) as usize };
            if count > MAX_RESULTS {
                bail!(
                    "Metal result buffer overflow: {count} shares in one {}-nonce batch",
                    self.batch_size
                );
            }
            let results = unsafe {
                std::slice::from_raw_parts(self.scratch.results.contents() as *const u64, count)
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

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn cache_round_trip_and_corruption_are_safe() {
            let directory =
                env::temp_dir().join(format!("blake2b-metal-cache-test-{}", std::process::id()));
            let path = directory.join("cache.json");
            let selection = KernelSelection {
                variant: MetalKernel::ScalarSplit,
                nonces_per_thread: 2,
                threadgroup_size: 64,
            };
            write_cached_selection(&path, "device-key".to_owned(), selection).unwrap();
            assert_eq!(read_cached_selection(&path, "device-key"), Some(selection));
            fs::write(&path, b"not json").unwrap();
            assert_eq!(read_cached_selection(&path, "device-key"), None);
            fs::remove_file(&path).unwrap();
            fs::remove_dir(&directory).unwrap();
        }
        #[test]
        fn threadgroups_follow_pipeline_limits() {
            assert_eq!(
                threadgroup_candidates(32, 256, None).unwrap(),
                [32, 64, 128, 256]
            );
            assert_eq!(threadgroup_candidates(32, 256, Some(96)).unwrap(), [96]);
            assert!(threadgroup_candidates(32, 256, Some(48)).is_err());
            assert!(threadgroup_candidates(32, 256, Some(512)).is_err());
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::super::{Backend, DeviceInfo, MetalOptions, PreparedJob};
    use crate::protocol::JobSpec;
    use anyhow::{bail, Result};
    pub fn devices() -> Result<Vec<DeviceInfo>> {
        bail!("Metal GPU mining requires macOS")
    }
    pub struct MetalBackend {
        info: DeviceInfo,
    }
    impl MetalBackend {
        pub fn new(_batch_size: u32, _options: MetalOptions) -> Result<Self> {
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

pub use imp::{devices, MetalBackend};
