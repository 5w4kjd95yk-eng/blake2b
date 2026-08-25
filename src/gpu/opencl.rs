#[cfg(feature = "opencl")]
mod imp {
    use std::{
        env, fs,
        mem::size_of,
        path::{Path, PathBuf},
        ptr,
        time::Instant,
    };

    use anyhow::{bail, Context as _, Result};
    use opencl3::{
        command_queue::{CommandQueue, CL_QUEUE_PROFILING_ENABLE},
        context::Context,
        device::{get_all_devices, Device, CL_DEVICE_TYPE_GPU},
        kernel::{ExecuteKernel, Kernel},
        memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_READ_WRITE, CL_MEM_WRITE_ONLY},
        program::Program,
        types::{CL_BLOCKING, CL_NON_BLOCKING},
    };
    use serde::{Deserialize, Serialize};

    use super::super::OpenClOptions;
    use super::super::{Backend, DeviceInfo, PreparedJob};
    use crate::config::{OpenClKernel, OpenClTuning};
    use crate::hash;
    use crate::protocol::JobSpec;

    const MAX_RESULTS: usize = 256;
    const KERNEL_SOURCE: &str = include_str!("../blake2b_tuned.cl");
    const TUNING_REVISION: u32 = 2;
    const TUNING_NONCES: u64 = 1 << 22;
    const VALIDATION_NONCES: u64 = 1_027;

    #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct KernelSelection {
        variant: OpenClKernel,
        nonces_per_item: u32,
        local_size: Option<usize>,
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

    fn selection_matches_options(selection: KernelSelection, options: OpenClOptions) -> bool {
        options
            .kernel
            .is_none_or(|value| value == selection.variant)
            && options
                .nonces_per_item
                .is_none_or(|value| value == selection.nonces_per_item)
            && options
                .local_size
                .is_none_or(|value| Some(value) == selection.local_size)
    }

    fn source_fingerprint() -> u64 {
        KERNEL_SOURCE
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    }

    fn tuning_key(device: &Device) -> Result<String> {
        Ok(format!(
            "revision={TUNING_REVISION};source={:016x};vendor={};device={};device_version={};driver={}",
            source_fingerprint(),
            device.vendor().context("read OpenCL device vendor")?,
            device.name().context("read OpenCL device name")?,
            device.version().context("read OpenCL device version")?,
            device
                .driver_version()
                .context("read OpenCL driver version")?,
        ))
    }

    fn cache_path() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        let root = env::var_os("HOME")
            .map(PathBuf::from)?
            .join("Library/Caches");
        #[cfg(target_os = "windows")]
        let root = env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let root = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(root.join("blake2b-miner/opencl-tuning.json"))
    }

    fn read_cached_selection(path: &Path, key: &str) -> Option<KernelSelection> {
        let contents = fs::read(path).ok()?;
        let cache: TuningCache = serde_json::from_slice(&contents).ok()?;
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
        let parent = path.parent().context("OpenCL tuning cache has no parent")?;
        fs::create_dir_all(parent).context("create OpenCL tuning cache directory")?;
        let temporary = parent.join(format!(".opencl-tuning-{}.tmp", std::process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(&cache)?)
            .context("write temporary OpenCL tuning cache")?;
        fs::rename(&temporary, path).context("replace OpenCL tuning cache")?;
        Ok(())
    }

    fn compile_kernel(context: &Context, selection: KernelSelection) -> Result<Kernel> {
        let build_options = selection.build_options();
        let program = Program::create_and_build_from_source(context, KERNEL_SOURCE, &build_options)
            .map_err(|error| anyhow::anyhow!("compile OpenCL Blake2b kernel: {error}"))?;
        Kernel::create(&program, "blake2b_datum_mine").context("create OpenCL Blake2b kernel")
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
        generation: u64,
    }

    impl OpenClJob {
        fn new(spec: &JobSpec, generation: u64) -> Result<Self> {
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
                generation,
            })
        }
    }

    struct TuningScratch {
        words: Buffer<u64>,
        counters: Buffer<u32>,
        results: Buffer<u64>,
    }

    impl TuningScratch {
        fn new(context: &Context) -> Result<Self> {
            unsafe {
                Ok(Self {
                    words: Buffer::create(context, CL_MEM_READ_ONLY, 16, ptr::null_mut())
                        .context("allocate OpenCL tuning job buffer")?,
                    counters: Buffer::create(context, CL_MEM_READ_WRITE, 2, ptr::null_mut())
                        .context("allocate OpenCL tuning counter buffer")?,
                    results: Buffer::create(
                        context,
                        CL_MEM_WRITE_ONLY,
                        MAX_RESULTS,
                        ptr::null_mut(),
                    )
                    .context("allocate OpenCL tuning result buffer")?,
                })
            }
        }

        unsafe fn write_words(&mut self, queue: &CommandQueue, words: &[u64; 16]) -> Result<()> {
            unsafe {
                queue.enqueue_write_buffer(&mut self.words, CL_BLOCKING, 0, words, &[])?;
            }
            Ok(())
        }

        unsafe fn reset(&mut self, queue: &CommandQueue) -> Result<()> {
            unsafe {
                queue.enqueue_fill_buffer(
                    &mut self.counters,
                    &[0u32],
                    0,
                    size_of::<[u32; 2]>(),
                    &[],
                )?;
            }
            Ok(())
        }
    }

    fn global_work_size(nonce_count: u64, selection: KernelSelection) -> Result<usize> {
        let work_items = nonce_count.div_ceil(u64::from(selection.nonces_per_item));
        let work_items = usize::try_from(work_items)?;
        if let Some(local_size) = selection.local_size {
            work_items
                .div_ceil(local_size)
                .checked_mul(local_size)
                .context("OpenCL global work size overflow")
        } else {
            Ok(work_items)
        }
    }

    unsafe fn enqueue_kernel(
        queue: &CommandQueue,
        kernel: &Kernel,
        selection: KernelSelection,
        scratch: &TuningScratch,
        start_nonce: u64,
        nonce_count: u64,
        target_prefix: u64,
    ) -> Result<opencl3::event::Event> {
        let mut execution = ExecuteKernel::new(kernel);
        execution
            .set_arg(&scratch.words)
            .set_arg(&start_nonce)
            .set_arg(&nonce_count)
            .set_arg(&target_prefix)
            .set_arg(&scratch.counters)
            .set_arg(&scratch.results)
            .set_arg(&(MAX_RESULTS as u32))
            .set_global_work_size(global_work_size(nonce_count, selection)?);
        if let Some(local_size) = selection.local_size {
            execution.set_local_work_size(local_size);
        }
        unsafe {
            execution
                .enqueue_nd_range(queue)
                .map_err(anyhow::Error::from)
        }
    }

    fn validation_case() -> ([u64; 16], u64, u64, Vec<u64>) {
        let mut blob = [0x5au8; 80];
        let mut block = [0u8; 128];
        block[..80].copy_from_slice(&blob);
        let mut words = [0u64; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(8)) {
            *word = u64::from_le_bytes(bytes.try_into().unwrap());
        }
        let start_nonce = u64::from(u32::MAX) - 511;
        let mut prefixes = (0..VALIDATION_NONCES)
            .map(|offset| {
                let nonce = start_nonce.wrapping_add(offset);
                blob[32..40].copy_from_slice(&nonce.to_le_bytes());
                let hash = hash::blake2b256(&blob);
                (nonce, u64::from_be_bytes(hash[..8].try_into().unwrap()))
            })
            .collect::<Vec<_>>();
        prefixes.sort_unstable_by_key(|(_, prefix)| *prefix);
        let target_prefix = prefixes[31].1;
        let mut expected = prefixes
            .into_iter()
            .filter_map(|(nonce, prefix)| (prefix <= target_prefix).then_some(nonce))
            .collect::<Vec<_>>();
        expected.sort_unstable();
        (words, start_nonce, target_prefix, expected)
    }

    fn validate_candidate(
        queue: &CommandQueue,
        kernel: &Kernel,
        selection: KernelSelection,
        scratch: &mut TuningScratch,
    ) -> Result<()> {
        let (words, start_nonce, target_prefix, expected) = validation_case();
        unsafe {
            scratch.write_words(queue, &words)?;
            scratch.reset(queue)?;
            enqueue_kernel(
                queue,
                kernel,
                selection,
                scratch,
                start_nonce,
                VALIDATION_NONCES,
                target_prefix,
            )?;
        }
        let mut summary = [0u32; 2];
        unsafe {
            queue.enqueue_read_buffer(&scratch.counters, CL_BLOCKING, 0, &mut summary, &[])?;
        }
        if summary[1] != 0 || summary[0] as usize > MAX_RESULTS {
            bail!("OpenCL tuning validation result buffer overflow");
        }
        let mut actual = vec![0u64; summary[0] as usize];
        if !actual.is_empty() {
            unsafe {
                queue.enqueue_read_buffer(&scratch.results, CL_BLOCKING, 0, &mut actual, &[])?;
            }
        }
        actual.sort_unstable();
        if actual != expected {
            bail!("OpenCL tuning candidate failed DATUM validation");
        }
        Ok(())
    }

    fn measure_candidate(
        queue: &CommandQueue,
        kernel: &Kernel,
        selection: KernelSelection,
        scratch: &mut TuningScratch,
    ) -> Result<u64> {
        let words = [0x5a5a_5a5a_5a5a_5a5a; 16];
        unsafe {
            scratch.write_words(queue, &words)?;
        }
        let mut samples = Vec::with_capacity(5);
        for iteration in 0u64..7 {
            unsafe {
                scratch.reset(queue)?;
                let wall_start = Instant::now();
                let event = enqueue_kernel(
                    queue,
                    kernel,
                    selection,
                    scratch,
                    iteration * TUNING_NONCES,
                    TUNING_NONCES,
                    0,
                )?;
                event.wait()?;
                let wall_nanoseconds = u64::try_from(wall_start.elapsed().as_nanos())?;
                if iteration >= 2 {
                    let event_nanoseconds =
                        event.profiling_command_end()? - event.profiling_command_start()?;
                    let measured = if event_nanoseconds >= wall_nanoseconds / 4
                        && event_nanoseconds <= wall_nanoseconds.saturating_mul(2)
                    {
                        event_nanoseconds
                    } else {
                        wall_nanoseconds
                    };
                    samples.push(measured);
                }
            }
        }
        samples.sort_unstable();
        Ok(samples[2])
    }

    fn validate_local_size(
        kernel: &Kernel,
        device: &Device,
        selection: KernelSelection,
    ) -> Result<()> {
        if let Some(local_size) = selection.local_size {
            let maximum = kernel
                .get_work_group_size(device.id())
                .context("query OpenCL kernel work-group limit")?;
            if local_size > maximum {
                bail!("OpenCL local size {local_size} exceeds kernel limit {maximum}");
            }
        }
        Ok(())
    }

    fn local_size_candidates(
        kernel: &Kernel,
        device: &Device,
        requested: Option<usize>,
    ) -> Result<Vec<Option<usize>>> {
        if let Some(local_size) = requested {
            let selection = KernelSelection {
                variant: OpenClKernel::Baseline,
                nonces_per_item: 1,
                local_size: Some(local_size),
            };
            validate_local_size(kernel, device, selection)?;
            return Ok(vec![Some(local_size)]);
        }
        let maximum = kernel
            .get_work_group_size(device.id())
            .context("query OpenCL kernel work-group limit")?;
        let preferred = kernel
            .get_work_group_size_multiple(device.id())
            .context("query preferred OpenCL work-group multiple")?
            .max(1);
        let mut candidates = vec![None];
        let mut local_size = preferred;
        while local_size <= maximum {
            candidates.push(Some(local_size));
            let Some(next) = local_size.checked_mul(2) else {
                break;
            };
            local_size = next;
        }
        if candidates.last() != Some(&Some(maximum)) {
            candidates.push(Some(maximum));
        }
        candidates.sort_unstable();
        candidates.dedup();
        Ok(candidates)
    }

    fn choose_kernel(
        context: &Context,
        device: &Device,
        options: OpenClOptions,
    ) -> Result<(KernelSelection, Kernel)> {
        if options.tuning == OpenClTuning::Off {
            let selection = KernelSelection::from_options(options);
            let kernel = compile_kernel(context, selection)?;
            validate_local_size(&kernel, device, selection)?;
            return Ok((selection, kernel));
        }

        let key = format!(
            "{};requested_kernel={:?};requested_width={:?};requested_local={:?}",
            tuning_key(device)?,
            options.kernel,
            options.nonces_per_item,
            options.local_size
        );
        let path = cache_path();
        if options.tuning == OpenClTuning::Auto {
            if let Some(selection) = path
                .as_deref()
                .and_then(|path| read_cached_selection(path, &key))
                .filter(|selection| selection_matches_options(*selection, options))
            {
                if let Ok(kernel) = compile_kernel(context, selection) {
                    if validate_local_size(&kernel, device, selection).is_ok() {
                        eprintln!("OpenCL tuning cache hit");
                        return Ok((selection, kernel));
                    }
                }
                eprintln!("OpenCL cached tuning selection is unusable; retuning");
            }
        }

        let queue = CommandQueue::create_default(context, CL_QUEUE_PROFILING_ENABLE)
            .context("create OpenCL profiling queue")?;
        let mut scratch = TuningScratch::new(context)?;
        let variants = options.kernel.map_or_else(
            || {
                vec![
                    OpenClKernel::Baseline,
                    OpenClKernel::ScalarSplit,
                    OpenClKernel::ScalarNative,
                ]
            },
            |variant| vec![variant],
        );
        let widths = options
            .nonces_per_item
            .map_or_else(|| vec![1, 2, 4], |width| vec![width]);
        let mut best: Option<(u64, KernelSelection, Kernel)> = None;

        for variant in variants {
            for nonces_per_item in widths.iter().copied() {
                let base = KernelSelection {
                    variant,
                    nonces_per_item,
                    local_size: None,
                };
                let kernel = match compile_kernel(context, base) {
                    Ok(kernel) => kernel,
                    Err(error) => {
                        eprintln!(
                            "OpenCL tuning skipped {variant:?} x{nonces_per_item}: {error:#}"
                        );
                        continue;
                    }
                };
                let local_sizes = match local_size_candidates(&kernel, device, options.local_size) {
                    Ok(local_sizes) => local_sizes,
                    Err(error) => {
                        eprintln!(
                            "OpenCL tuning skipped {variant:?} x{nonces_per_item}: {error:#}"
                        );
                        continue;
                    }
                };
                let mut best_for_kernel = None;
                for local_size in local_sizes {
                    let selection = KernelSelection { local_size, ..base };
                    let result = validate_candidate(&queue, &kernel, selection, &mut scratch)
                        .and_then(|()| measure_candidate(&queue, &kernel, selection, &mut scratch));
                    match result {
                        Ok(nanoseconds) => {
                            let mhps = TUNING_NONCES as f64 / nanoseconds as f64 * 1_000.0;
                            eprintln!(
                                "OpenCL tuning candidate kernel={variant:?} nonces_per_item={nonces_per_item} local_size={} {:.3} MH/s",
                                local_size.map_or_else(
                                    || "driver".to_owned(),
                                    |size| size.to_string()
                                ),
                                mhps
                            );
                            if best_for_kernel
                                .as_ref()
                                .is_none_or(|(best_time, _)| nanoseconds < *best_time)
                            {
                                best_for_kernel = Some((nanoseconds, selection));
                            }
                        }
                        Err(error) => eprintln!(
                            "OpenCL tuning rejected {variant:?} x{nonces_per_item} local={local_size:?}: {error:#}"
                        ),
                    }
                }
                if let Some((nanoseconds, selection)) = best_for_kernel {
                    if best
                        .as_ref()
                        .is_none_or(|(best_time, _, _)| nanoseconds < *best_time)
                    {
                        best = Some((nanoseconds, selection, kernel));
                    }
                }
            }
        }

        let (_, selection, kernel) = best.context("no valid OpenCL tuning candidate")?;
        if let Some(path) = path {
            if let Err(error) = write_cached_selection(&path, key, selection) {
                eprintln!("OpenCL tuning cache was not written: {error:#}");
            }
        }
        Ok((selection, kernel))
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
        loaded_generation: Option<u64>,
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
            let (selection, kernel) = choose_kernel(&context, &device, options)?;
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
                loaded_generation: None,
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

        fn prepare_job(&self, spec: &JobSpec, generation: u64) -> Result<Box<dyn PreparedJob>> {
            Ok(Box::new(OpenClJob::new(spec, generation)?))
        }

        fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>> {
            let job = job
                .as_any()
                .downcast_ref::<OpenClJob>()
                .context("prepared job does not belong to the OpenCL backend")?;
            unsafe {
                if self.loaded_generation != Some(job.generation) {
                    self.queue.enqueue_write_buffer(
                        &mut self.words,
                        CL_NON_BLOCKING,
                        0,
                        &job.words,
                        &[],
                    )?;
                    self.loaded_generation = Some(job.generation);
                }
                self.queue.enqueue_fill_buffer(
                    &mut self.counters,
                    &[0u32],
                    0,
                    size_of::<[u32; 2]>(),
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
                execution.enqueue_nd_range(&self.queue)?;
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

    #[cfg(test)]
    mod tuning_tests {
        use super::*;

        #[test]
        fn cache_round_trip_and_corruption_are_safe() {
            let directory =
                env::temp_dir().join(format!("blake2b-opencl-cache-test-{}", std::process::id()));
            let path = directory.join("cache.json");
            let selection = KernelSelection {
                variant: OpenClKernel::ScalarSplit,
                nonces_per_item: 2,
                local_size: Some(64),
            };

            write_cached_selection(&path, "device-key".to_owned(), selection).unwrap();
            assert_eq!(read_cached_selection(&path, "device-key"), Some(selection));
            fs::write(&path, b"not json").unwrap();
            assert_eq!(read_cached_selection(&path, "device-key"), None);

            fs::remove_file(&path).unwrap();
            fs::remove_dir(&directory).unwrap();
        }

        #[test]
        fn global_size_rounds_up_for_vector_width_and_local_size() {
            let selection = KernelSelection {
                variant: OpenClKernel::Baseline,
                nonces_per_item: 4,
                local_size: Some(32),
            };
            assert_eq!(global_work_size(1_027, selection).unwrap(), 288);
        }
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
