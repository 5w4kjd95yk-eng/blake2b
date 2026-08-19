use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use arc_swap::ArcSwapOption;
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::Value;

use crate::{
    config::{ByteOrder, Config, Endpoint, Mode},
    gpu,
    hash::PreparedBlock,
    protocol::{self, JobSpec, SessionState, Submit},
    target::Target,
};

const NONCES_PER_RESERVATION: u64 = 16_384;
const STALE_CHECK_INTERVAL: u64 = 1_024;

struct Work {
    spec: JobSpec,
    prepared: PreparedBlock,
    epoch: u64,
    next_nonce: AtomicU64,
}

impl Work {
    fn new(spec: JobSpec, epoch: u64) -> Result<Self> {
        let prepared = PreparedBlock::new(
            &spec.blob,
            spec.nonce_offset,
            spec.nonce_size,
            spec.nonce_order == ByteOrder::Little,
        )
        .context("job cannot be prepared for one-block SIMD hashing")?;
        Ok(Self {
            spec,
            prepared,
            epoch,
            next_nonce: AtomicU64::new(0),
        })
    }
}

struct Share {
    work: Arc<Work>,
    nonce: u64,
}

#[derive(Default)]
struct HashCounters {
    cpu: AtomicU64,
    gpu: AtomicU64,
}

impl HashCounters {
    fn snapshot(&self) -> (u64, u64) {
        (
            self.cpu.load(Ordering::Relaxed),
            self.gpu.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone)]
struct WorkerContext {
    current: Arc<ArcSwapOption<Work>>,
    active_epoch: Arc<AtomicU64>,
    hashes: Arc<HashCounters>,
    stop: Arc<AtomicBool>,
    gpu_failed: Arc<AtomicBool>,
    shares: Sender<Share>,
}

pub fn run(config: Config) -> Result<()> {
    if config.benchmark {
        return benchmark(&config);
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stop);
    ctrlc::set_handler(move || stop_signal.store(true, Ordering::Release))
        .context("install Ctrl-C handler")?;

    let gpu_backend = if config.device.uses_gpu() {
        let backend = gpu::Miner::new(config.gpu_batch_size)?;
        eprintln!(
            "Metal GPU: {} batch_size={}",
            backend.device_name(),
            backend.batch_size()
        );
        Some(backend)
    } else {
        None
    };
    let current = Arc::new(ArcSwapOption::<Work>::empty());
    let active_epoch = Arc::new(AtomicU64::new(0));
    let hashes = Arc::new(HashCounters::default());
    let gpu_failed = Arc::new(AtomicBool::new(false));
    let (shares_tx, shares_rx) = unbounded();
    let workers = spawn_workers(
        &config,
        gpu_backend,
        WorkerContext {
            current: Arc::clone(&current),
            active_epoch: Arc::clone(&active_epoch),
            hashes: Arc::clone(&hashes),
            stop: Arc::clone(&stop),
            gpu_failed: Arc::clone(&gpu_failed),
            shares: shares_tx,
        },
    )?;

    eprintln!(
        "mode={} device={:?} endpoint={}:{} cpu_threads={} simd={}",
        match config.mode {
            Mode::Sia => "sia",
            Mode::Datum => "datum",
            Mode::Normal => "normal",
        },
        config.device,
        config.endpoint.host,
        config.endpoint.port,
        if config.device.uses_cpu() {
            config.threads
        } else {
            0
        },
        if cfg!(target_arch = "aarch64") {
            "neon-4way"
        } else {
            "scalar-4way"
        }
    );

    while !stop.load(Ordering::Acquire) {
        if let Err(error) =
            run_session(&config, &current, &active_epoch, &hashes, &stop, &shares_rx)
        {
            current.store(None);
            active_epoch.fetch_add(1, Ordering::AcqRel);
            if !stop.load(Ordering::Acquire) {
                eprintln!("Stratum disconnected: {error:#}");
                interruptible_sleep(config.reconnect_delay, &stop);
            }
        }
    }

    current.store(None);
    active_epoch.fetch_add(1, Ordering::AcqRel);
    for worker in workers {
        let _ = worker.join();
    }
    if gpu_failed.load(Ordering::Acquire) {
        bail!("Metal GPU worker failed");
    }
    Ok(())
}

fn spawn_workers(
    config: &Config,
    gpu_backend: Option<gpu::Miner>,
    context: WorkerContext,
) -> Result<Vec<thread::JoinHandle<()>>> {
    let mut workers = Vec::new();
    if config.device.uses_cpu() {
        for index in 0..config.threads {
            let context = context.clone();
            let worker = thread::Builder::new()
                .name(format!("blake2b-{index}"))
                .spawn(move || worker_loop(context))
                .with_context(|| format!("spawn CPU worker {index}"))?;
            workers.push(worker);
        }
    }
    if let Some(gpu_backend) = gpu_backend {
        let context = context.clone();
        let worker = thread::Builder::new()
            .name("blake2b-metal".to_owned())
            .spawn(move || {
                if let Err(error) = gpu_worker_loop(gpu_backend, &context) {
                    eprintln!("Metal GPU failed: {error:#}");
                    context.gpu_failed.store(true, Ordering::Release);
                    context.stop.store(true, Ordering::Release);
                }
            })
            .context("spawn Metal GPU worker")?;
        workers.push(worker);
    }
    Ok(workers)
}

fn worker_loop(context: WorkerContext) {
    let mut pending_hashes = 0u64;
    while !context.stop.load(Ordering::Relaxed) {
        let Some(work) = context.current.load_full() else {
            flush_hashes(&context.hashes.cpu, &mut pending_hashes);
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        let start = work
            .next_nonce
            .fetch_add(NONCES_PER_RESERVATION, Ordering::Relaxed);
        let mut offset = 0;
        while offset < NONCES_PER_RESERVATION {
            if offset % STALE_CHECK_INTERVAL == 0
                && context.active_epoch.load(Ordering::Relaxed) != work.epoch
            {
                break;
            }
            let nonce = start.wrapping_add(offset);
            let digests = work.prepared.hash4(nonce);
            pending_hashes += 4;
            for (lane, digest) in digests.iter().enumerate() {
                if work.spec.target.accepts(digest, work.spec.hash_order) {
                    let _ = context.shares.send(Share {
                        work: Arc::clone(&work),
                        nonce: nonce.wrapping_add(lane as u64),
                    });
                }
            }
            offset += 4;
        }
        if pending_hashes >= NONCES_PER_RESERVATION {
            flush_hashes(&context.hashes.cpu, &mut pending_hashes);
        }
    }
    flush_hashes(&context.hashes.cpu, &mut pending_hashes);
}

fn gpu_worker_loop(mut miner: gpu::Miner, context: &WorkerContext) -> Result<()> {
    let mut cached_job: Option<(u64, gpu::Job)> = None;
    while !context.stop.load(Ordering::Relaxed) {
        let Some(work) = context.current.load_full() else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        if cached_job.as_ref().map(|(epoch, _)| *epoch) != Some(work.epoch) {
            cached_job = Some((work.epoch, gpu::Job::new(&work.spec)?));
        }
        let start = work
            .next_nonce
            .fetch_add(u64::from(miner.batch_size()), Ordering::Relaxed);
        let winning_nonces = miner.mine(&cached_job.as_ref().unwrap().1, start)?;
        context
            .hashes
            .gpu
            .fetch_add(u64::from(miner.batch_size()), Ordering::Relaxed);
        if context.active_epoch.load(Ordering::Acquire) != work.epoch {
            continue;
        }
        for nonce in winning_nonces {
            let _ = context.shares.send(Share {
                work: Arc::clone(&work),
                nonce,
            });
        }
    }
    Ok(())
}

fn flush_hashes(hashes: &AtomicU64, pending: &mut u64) {
    if *pending != 0 {
        hashes.fetch_add(*pending, Ordering::Relaxed);
        *pending = 0;
    }
}

fn run_session(
    config: &Config,
    current: &ArcSwapOption<Work>,
    active_epoch: &AtomicU64,
    hashes: &HashCounters,
    stop: &AtomicBool,
    shares: &Receiver<Share>,
) -> Result<()> {
    let stream = connect(&config.endpoint)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(25)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut stream = BufReader::new(stream);
    write_message(stream.get_mut(), &protocol::subscribe_request())?;
    write_message(
        stream.get_mut(),
        &protocol::authorize_request(&config.username, &config.password),
    )?;

    eprintln!("Stratum connected");
    let mut session = SessionState::default();
    let mut request_id = 10u64;
    let mut accepted = 0u64;
    let mut rejected = 0u64;
    let (mut last_cpu, mut last_gpu) = hashes.snapshot();
    let mut last_stats = Instant::now();
    let mut line = String::new();

    while !stop.load(Ordering::Acquire) {
        while let Ok(share) = shares.try_recv() {
            if share.work.epoch != active_epoch.load(Ordering::Acquire) {
                continue;
            }
            let nonce = share.work.prepared.nonce_hex(share.nonce);
            let message = share
                .work
                .spec
                .submission(&config.username, request_id, nonce);
            write_message(stream.get_mut(), &message)?;
            request_id = request_id.wrapping_add(1).max(10);
        }

        line.clear();
        match stream.read_line(&mut line) {
            Ok(0) => bail!("pool closed the connection"),
            Ok(_) => {
                let message: Value =
                    serde_json::from_str(&line).context("invalid JSON from pool")?;
                handle_message(
                    message,
                    config,
                    &mut session,
                    current,
                    active_epoch,
                    &mut accepted,
                    &mut rejected,
                )?;
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(error).context("read Stratum connection"),
        }

        if last_stats.elapsed() >= config.stats_interval {
            let now = Instant::now();
            let (cpu, gpu) = hashes.snapshot();
            let seconds = now.duration_since(last_stats).as_secs_f64();
            let cpu_rate = cpu.saturating_sub(last_cpu) as f64 / seconds;
            let gpu_rate = gpu.saturating_sub(last_gpu) as f64 / seconds;
            let total_rate = cpu_rate + gpu_rate;
            eprintln!(
                "{:.3} MH/s (cpu={:.3} gpu={:.3}) accepted={} rejected={} total_hashes={}",
                total_rate / 1_000_000.0,
                cpu_rate / 1_000_000.0,
                gpu_rate / 1_000_000.0,
                accepted,
                rejected,
                cpu + gpu
            );
            last_cpu = cpu;
            last_gpu = gpu;
            last_stats = now;
        }
    }
    Ok(())
}

fn handle_message(
    message: Value,
    config: &Config,
    session: &mut SessionState,
    current: &ArcSwapOption<Work>,
    active_epoch: &AtomicU64,
    accepted: &mut u64,
    rejected: &mut u64,
) -> Result<()> {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message.get("params").unwrap_or(&Value::Null);
        match method {
            "mining.notify" => {
                let spec = session.parse_job(params, config)?;
                install_work(spec, current, active_epoch)?;
            }
            "mining.set_target" | "mining.set_difficulty" => {
                if session.apply_target(method, params, config.mode)? {
                    if let (Some(work), Some(target)) =
                        (current.load_full(), session.target.clone())
                    {
                        let mut spec = work.spec.clone();
                        spec.target = target;
                        install_work(spec, current, active_epoch)?;
                    }
                    eprintln!("share target updated");
                } else {
                    eprintln!("ignored mining.set_difficulty in --normal mode; send mining.set_target or a job target");
                }
            }
            "client.reconnect" => bail!("pool requested reconnect"),
            "client.show_message" => eprintln!("pool: {params}"),
            _ => {}
        }
        return Ok(());
    }

    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return Ok(());
    };
    if let Some(error) = message.get("error").filter(|error| !error.is_null()) {
        if id >= 10 {
            *rejected += 1;
            eprintln!("share rejected: {error}");
            return Ok(());
        }
        bail!("Stratum request {id} failed: {error}");
    }
    match id {
        1 => session.apply_subscribe_response(&message, config.mode),
        2 if message.get("result") != Some(&Value::Bool(true)) => {
            bail!("pool rejected mining.authorize")
        }
        2 => Ok(()),
        _ if id >= 10 => {
            if message.get("result") == Some(&Value::Bool(true)) {
                *accepted += 1;
                eprintln!("share accepted ({accepted})");
            } else {
                *rejected += 1;
                eprintln!(
                    "share rejected: {}",
                    message.get("result").unwrap_or(&Value::Null)
                );
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn install_work(
    spec: JobSpec,
    current: &ArcSwapOption<Work>,
    active_epoch: &AtomicU64,
) -> Result<()> {
    let job_id = spec.id.clone();
    let epoch = active_epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    current.store(Some(Arc::new(Work::new(spec, epoch)?)));
    eprintln!("new job {job_id}");
    Ok(())
}

fn write_message(stream: &mut TcpStream, message: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stream, message)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn connect(endpoint: &Endpoint) -> Result<TcpStream> {
    let addresses = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .with_context(|| format!("resolve {}:{}", endpoint.host, endpoint.port))?;
    let mut last_error = None;
    for address in addresses {
        match connect_address(address) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error)
            .with_context(|| format!("connect to {}:{}", endpoint.host, endpoint.port));
    }
    bail!("host resolved to no addresses")
}

fn connect_address(address: SocketAddr) -> std::io::Result<TcpStream> {
    TcpStream::connect_timeout(&address, Duration::from_secs(10))
}

fn interruptible_sleep(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn benchmark(config: &Config) -> Result<()> {
    let blob = [0x5au8; 80];
    let (offset, size, nonce_order, hash_order) = match config.mode {
        Mode::Sia | Mode::Datum => (32, 8, ByteOrder::Little, ByteOrder::Big),
        Mode::Normal => (
            config.nonce_offset,
            config.nonce_size,
            config.nonce_endian,
            config.hash_byte_order,
        ),
    };
    let spec = JobSpec {
        id: "benchmark".to_owned(),
        blob: blob.to_vec(),
        target: Target::from_hex("00")?,
        nonce_offset: offset,
        nonce_size: size,
        nonce_order,
        hash_order,
        submit: Submit::Normal,
    };
    let current = Arc::new(ArcSwapOption::from(Some(Arc::new(Work::new(spec, 1)?))));
    let active_epoch = Arc::new(AtomicU64::new(1));
    let hashes = Arc::new(HashCounters::default());
    let stop = Arc::new(AtomicBool::new(false));
    let gpu_failed = Arc::new(AtomicBool::new(false));
    let (shares, _unused_receiver) = unbounded();
    let gpu_backend = if config.device.uses_gpu() {
        let backend = gpu::Miner::new(config.gpu_batch_size)?;
        eprintln!(
            "Metal GPU: {} batch_size={}",
            backend.device_name(),
            backend.batch_size()
        );
        Some(backend)
    } else {
        None
    };
    let workers = spawn_workers(
        config,
        gpu_backend,
        WorkerContext {
            current: Arc::clone(&current),
            active_epoch: Arc::clone(&active_epoch),
            hashes: Arc::clone(&hashes),
            stop: Arc::clone(&stop),
            gpu_failed: Arc::clone(&gpu_failed),
            shares,
        },
    )?;
    let duration = Duration::from_secs(3);
    let start = Instant::now();
    thread::sleep(duration);
    stop.store(true, Ordering::Release);
    active_epoch.store(2, Ordering::Release);
    for worker in workers {
        let _ = worker.join();
    }
    let elapsed = start.elapsed();
    if gpu_failed.load(Ordering::Acquire) {
        bail!("Metal GPU benchmark failed");
    }
    let (cpu, gpu) = hashes.snapshot();
    let total = cpu + gpu;
    eprintln!(
        "benchmark: {:.3} MH/s (cpu={:.3} gpu={:.3}), {} hashes in {:.3}s, device={:?}",
        total as f64 / elapsed.as_secs_f64() / 1_000_000.0,
        cpu as f64 / elapsed.as_secs_f64() / 1_000_000.0,
        gpu as f64 / elapsed.as_secs_f64() / 1_000_000.0,
        total,
        elapsed.as_secs_f64(),
        config.device,
    );
    Ok(())
}
