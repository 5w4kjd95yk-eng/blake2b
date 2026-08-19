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
    hash::PreparedBlock,
    protocol::{self, JobSpec, SessionState},
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

pub fn run(config: Config) -> Result<()> {
    if config.benchmark {
        return benchmark(&config);
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stop);
    ctrlc::set_handler(move || stop_signal.store(true, Ordering::Release))
        .context("install Ctrl-C handler")?;

    let current = Arc::new(ArcSwapOption::<Work>::empty());
    let active_epoch = Arc::new(AtomicU64::new(0));
    let hashes = Arc::new(AtomicU64::new(0));
    let (shares_tx, shares_rx) = unbounded();
    let workers = spawn_workers(
        config.threads,
        Arc::clone(&current),
        Arc::clone(&active_epoch),
        Arc::clone(&hashes),
        Arc::clone(&stop),
        shares_tx,
    )?;

    eprintln!(
        "mode={} endpoint={}:{} threads={} simd={}",
        match config.mode {
            Mode::Sia => "sia",
            Mode::Normal => "normal",
        },
        config.endpoint.host,
        config.endpoint.port,
        config.threads,
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
    Ok(())
}

fn spawn_workers(
    count: usize,
    current: Arc<ArcSwapOption<Work>>,
    active_epoch: Arc<AtomicU64>,
    hashes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    shares: Sender<Share>,
) -> Result<Vec<thread::JoinHandle<()>>> {
    (0..count)
        .map(|index| {
            let current = Arc::clone(&current);
            let active_epoch = Arc::clone(&active_epoch);
            let hashes = Arc::clone(&hashes);
            let stop = Arc::clone(&stop);
            let shares = shares.clone();
            thread::Builder::new()
                .name(format!("blake2b-{index}"))
                .spawn(move || worker_loop(current, active_epoch, hashes, stop, shares))
                .with_context(|| format!("spawn worker {index}"))
        })
        .collect()
}

fn worker_loop(
    current: Arc<ArcSwapOption<Work>>,
    active_epoch: Arc<AtomicU64>,
    hashes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    shares: Sender<Share>,
) {
    let mut pending_hashes = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let Some(work) = current.load_full() else {
            flush_hashes(&hashes, &mut pending_hashes);
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        let start = work
            .next_nonce
            .fetch_add(NONCES_PER_RESERVATION, Ordering::Relaxed);
        let mut offset = 0;
        while offset < NONCES_PER_RESERVATION {
            if offset % STALE_CHECK_INTERVAL == 0
                && active_epoch.load(Ordering::Relaxed) != work.epoch
            {
                break;
            }
            let nonce = start.wrapping_add(offset);
            let digests = work.prepared.hash4(nonce);
            pending_hashes += 4;
            for (lane, digest) in digests.iter().enumerate() {
                if work.spec.target.accepts(digest, work.spec.hash_order) {
                    let _ = shares.send(Share {
                        work: Arc::clone(&work),
                        nonce: nonce.wrapping_add(lane as u64),
                    });
                }
            }
            offset += 4;
        }
        if pending_hashes >= NONCES_PER_RESERVATION {
            flush_hashes(&hashes, &mut pending_hashes);
        }
    }
    flush_hashes(&hashes, &mut pending_hashes);
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
    hashes: &AtomicU64,
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
    let mut last_total = hashes.load(Ordering::Relaxed);
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
            let total = hashes.load(Ordering::Relaxed);
            let rate = total.saturating_sub(last_total) as f64
                / now.duration_since(last_stats).as_secs_f64();
            eprintln!(
                "{:.3} MH/s accepted={} rejected={} total_hashes={}",
                rate / 1_000_000.0,
                accepted,
                rejected,
                total
            );
            last_total = total;
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
    let (offset, size, little_endian) = match config.mode {
        Mode::Sia => (32, 8, true),
        Mode::Normal => (
            config.nonce_offset,
            config.nonce_size,
            config.nonce_endian == ByteOrder::Little,
        ),
    };
    let prepared = Arc::new(
        PreparedBlock::new(&blob, offset, size, little_endian)
            .context("benchmark nonce layout does not fit its 80-byte blob")?,
    );
    let duration = Duration::from_secs(3);
    let start = Instant::now();
    let handles: Vec<_> = (0..config.threads)
        .map(|index| {
            let prepared = Arc::clone(&prepared);
            thread::spawn(move || {
                let mut hashes = 0u64;
                let mut nonce = (index as u64) << 48;
                while start.elapsed() < duration {
                    for _ in 0..256 {
                        std::hint::black_box(prepared.hash4(nonce));
                        nonce = nonce.wrapping_add(4);
                        hashes += 4;
                    }
                }
                hashes
            })
        })
        .collect();
    let total: u64 = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .sum();
    let elapsed = start.elapsed();
    eprintln!(
        "benchmark: {:.3} MH/s, {} hashes in {:.3}s, {} threads, {}",
        total as f64 / elapsed.as_secs_f64() / 1_000_000.0,
        total,
        elapsed.as_secs_f64(),
        config.threads,
        if cfg!(target_arch = "aarch64") {
            "NEON 4-way"
        } else {
            "scalar 4-way"
        }
    );
    Ok(())
}
