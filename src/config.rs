use std::{fs, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use percent_encoding::percent_decode_str;
use serde::{de, Deserialize, Deserializer, Serialize};
use url::Url;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceMode {
    #[default]
    Cpu,
    Gpu,
    Both,
}

impl DeviceMode {
    pub fn uses_cpu(self) -> bool {
        matches!(self, Self::Cpu | Self::Both)
    }

    pub fn uses_gpu(self) -> bool {
        matches!(self, Self::Gpu | Self::Both)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GpuBackend {
    #[default]
    Auto,
    Metal,
    Cuda,
    Opencl,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OpenClTuning {
    #[default]
    Auto,
    Off,
    Retune,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OpenClKernel {
    Baseline,
    ScalarSplit,
    ScalarNative,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CudaKernel {
    #[default]
    Reference,
    Scalar,
    ScalarPermute,
    ScalarPermutePrecompute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuDevices {
    All,
    Indices(Vec<usize>),
}

impl Default for GpuDevices {
    fn default() -> Self {
        Self::Indices(vec![0])
    }
}

impl FromStr for GpuDevices {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        if raw.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        let indices = raw
            .split(',')
            .map(|value| {
                value
                    .trim()
                    .parse::<usize>()
                    .with_context(|| format!("invalid GPU device index {value:?}"))
            })
            .collect::<Result<Vec<_>>>()?;
        if indices.is_empty() {
            bail!("gpu_devices must be an index, a comma-separated list, or all");
        }
        if indices
            .iter()
            .enumerate()
            .any(|(position, index)| indices[..position].contains(index))
        {
            bail!("gpu_devices must not contain duplicate indices");
        }
        Ok(Self::Indices(indices))
    }
}

impl<'de> Deserialize<'de> for GpuDevices {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Index(usize),
            Indices(Vec<usize>),
            Text(String),
        }

        match Value::deserialize(deserializer)? {
            Value::Index(index) => Ok(Self::Indices(vec![index])),
            Value::Indices(indices) if indices.is_empty() => {
                Err(de::Error::custom("gpu_devices list must not be empty"))
            }
            Value::Indices(indices) => Ok(Self::Indices(indices)),
            Value::Text(text) => text.parse().map_err(de::Error::custom),
        }
    }
}

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// YAML configuration file.
    #[arg(short, long, default_value = "config.yaml")]
    pub config: PathBuf,

    /// Stratum endpoint. The misspelled --startum-url is retained as an alias.
    #[arg(long, alias = "startum-url")]
    pub stratum_url: Option<String>,

    /// SOCKS5 proxy as host:port. Destination DNS is resolved by the proxy.
    #[arg(long)]
    pub socks5_proxy: Option<String>,

    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub password: Option<String>,

    /// Worker threads. Zero selects all logical CPUs.
    #[arg(short = 't', long)]
    pub threads: Option<usize>,

    /// Hashing device: cpu, gpu, or both.
    #[arg(long, value_enum)]
    pub device: Option<DeviceMode>,

    /// GPU implementation: auto, metal, cuda, or opencl.
    #[arg(long, value_enum)]
    pub gpu_backend: Option<GpuBackend>,

    /// GPU index, comma-separated indices, or all.
    #[arg(long)]
    pub gpu_devices: Option<GpuDevices>,

    /// Nonces dispatched in each GPU batch.
    #[arg(long)]
    pub gpu_batch_size: Option<u32>,

    /// Select the experimental CUDA DATUM kernel implementation.
    #[arg(long, value_enum)]
    pub cuda_kernel: Option<CudaKernel>,

    /// Number of sequential nonces computed by each CUDA thread.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4))]
    pub cuda_nonces_per_thread: Option<u32>,

    /// CUDA threads per block; must be a warp-sized multiple.
    #[arg(long)]
    pub cuda_block_size: Option<u32>,

    /// OpenCL kernel/work-group tuning policy.
    #[arg(long, value_enum)]
    pub opencl_tuning: Option<OpenClTuning>,

    /// Force an OpenCL kernel implementation instead of selecting it automatically.
    #[arg(long, value_enum)]
    pub opencl_kernel: Option<OpenClKernel>,

    /// Force an OpenCL local work-group size instead of selecting it automatically.
    #[arg(long)]
    pub opencl_local_size: Option<usize>,

    /// Force the number of nonces computed by each OpenCL work-item.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4))]
    pub opencl_nonces_per_item: Option<u32>,

    /// Hash locally instead of connecting to a pool.
    #[arg(long)]
    pub benchmark: bool,

    /// Duration of a local benchmark in seconds.
    #[arg(long, default_value_t = 3)]
    pub benchmark_seconds: u64,

    /// List usable GPU devices and exit without connecting to Stratum.
    #[arg(long)]
    pub list_devices: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    stratum_url: Option<String>,
    socks5_proxy: Option<String>,
    username: Option<String>,
    password: Option<String>,
    threads: Option<usize>,
    device: Option<DeviceMode>,
    gpu_backend: Option<GpuBackend>,
    gpu_devices: Option<GpuDevices>,
    gpu_batch_size: Option<u32>,
    cuda_kernel: Option<CudaKernel>,
    cuda_nonces_per_thread: Option<u32>,
    cuda_block_size: Option<u32>,
    opencl_tuning: Option<OpenClTuning>,
    opencl_kernel: Option<OpenClKernel>,
    opencl_local_size: Option<usize>,
    opencl_nonces_per_item: Option<u32>,
    reconnect_delay_seconds: Option<u64>,
    stats_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub endpoint: Endpoint,
    pub socks5_proxy: Option<Endpoint>,
    pub username: String,
    pub password: String,
    pub threads: usize,
    pub device: DeviceMode,
    pub gpu_backend: GpuBackend,
    pub gpu_devices: GpuDevices,
    pub gpu_batch_size: u32,
    pub cuda_kernel: CudaKernel,
    pub cuda_nonces_per_thread: u32,
    pub cuda_block_size: u32,
    pub opencl_tuning: OpenClTuning,
    pub opencl_kernel: Option<OpenClKernel>,
    pub opencl_local_size: Option<usize>,
    pub opencl_nonces_per_item: Option<u32>,
    pub reconnect_delay: Duration,
    pub stats_interval: Duration,
    pub benchmark: bool,
    pub benchmark_duration: Duration,
    pub list_devices: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
struct UrlParts {
    endpoint: Endpoint,
    username: Option<String>,
    password: Option<String>,
}

pub fn load(args: Args) -> Result<Config> {
    let file = if args.config.exists() {
        let contents = fs::read_to_string(&args.config)
            .with_context(|| format!("read {}", args.config.display()))?;
        serde_yml::from_str::<FileConfig>(&contents)
            .with_context(|| format!("parse {}", args.config.display()))?
    } else if args.stratum_url.is_some() || args.benchmark || args.list_devices {
        FileConfig::default()
    } else {
        bail!(
            "configuration file {} does not exist",
            args.config.display()
        );
    };

    let device = args.device.or(file.device).unwrap_or_default();
    let gpu_backend = args.gpu_backend.or(file.gpu_backend).unwrap_or_default();
    let gpu_devices = args.gpu_devices.or(file.gpu_devices).unwrap_or_default();
    let raw_url = args
        .stratum_url
        .or(file.stratum_url)
        .unwrap_or_else(|| "stratum+tcp://127.0.0.1:3333".to_owned());
    let url = parse_url(&raw_url)?;
    let socks5_proxy = args
        .socks5_proxy
        .or(file.socks5_proxy)
        .map(|value| parse_proxy(&value))
        .transpose()?;
    let username = args
        .username
        .or(url.username)
        .or(file.username)
        .unwrap_or_default();
    let password = args
        .password
        .or(url.password)
        .or(file.password)
        .unwrap_or_else(|| "x".to_owned());
    let requested_threads = args.threads.or(file.threads).unwrap_or(0);
    let threads = if requested_threads == 0 {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        if device == DeviceMode::Both {
            available.saturating_sub(1).max(1)
        } else {
            available
        }
    } else {
        requested_threads
    };
    let gpu_batch_size = args
        .gpu_batch_size
        .or(file.gpu_batch_size)
        .unwrap_or(1_048_576);
    if gpu_batch_size == 0 {
        bail!("gpu_batch_size must be greater than zero");
    }
    let cuda_nonces_per_thread = args
        .cuda_nonces_per_thread
        .or(file.cuda_nonces_per_thread)
        .unwrap_or(1);
    if !matches!(cuda_nonces_per_thread, 1 | 2 | 4) {
        bail!("cuda_nonces_per_thread must be 1, 2, or 4");
    }
    let cuda_block_size = args.cuda_block_size.or(file.cuda_block_size).unwrap_or(256);
    if !(32..=1024).contains(&cuda_block_size) || !cuda_block_size.is_multiple_of(32) {
        bail!("cuda_block_size must be a multiple of 32 from 32 through 1024");
    }
    let opencl_local_size = args.opencl_local_size.or(file.opencl_local_size);
    if opencl_local_size == Some(0) {
        bail!("opencl_local_size must be greater than zero");
    }
    if args.benchmark_seconds == 0 {
        bail!("benchmark_seconds must be greater than zero");
    }

    Ok(Config {
        endpoint: url.endpoint,
        socks5_proxy,
        username,
        password,
        threads,
        device,
        gpu_backend,
        gpu_devices,
        gpu_batch_size,
        cuda_kernel: args.cuda_kernel.or(file.cuda_kernel).unwrap_or_default(),
        cuda_nonces_per_thread,
        cuda_block_size,
        opencl_tuning: args
            .opencl_tuning
            .or(file.opencl_tuning)
            .unwrap_or_default(),
        opencl_kernel: args.opencl_kernel.or(file.opencl_kernel),
        opencl_local_size,
        opencl_nonces_per_item: args.opencl_nonces_per_item.or(file.opencl_nonces_per_item),
        reconnect_delay: Duration::from_secs(file.reconnect_delay_seconds.unwrap_or(5)),
        stats_interval: Duration::from_secs(file.stats_interval_seconds.unwrap_or(5).max(1)),
        benchmark: args.benchmark,
        benchmark_duration: Duration::from_secs(args.benchmark_seconds),
        list_devices: args.list_devices,
    })
}

fn parse_url(raw: &str) -> Result<UrlParts> {
    let url = Url::from_str(raw).with_context(|| format!("invalid Stratum URL {raw:?}"))?;
    if url.scheme() != "stratum+tcp" {
        bail!(
            "unsupported URL scheme {:?}; expected stratum+tcp",
            url.scheme()
        );
    }
    let host = url
        .host_str()
        .context("Stratum URL has no host")?
        .to_owned();
    let port = url.port().context("Stratum URL has no port")?;
    if url.path() != "" && url.path() != "/" {
        bail!("Stratum URL must not contain a path");
    }

    Ok(UrlParts {
        endpoint: Endpoint { host, port },
        username: (!url.username().is_empty()).then(|| decode(url.username())),
        password: url.password().map(decode),
    })
}

fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

fn parse_proxy(raw: &str) -> Result<Endpoint> {
    let url = Url::parse(&format!("socks5://{raw}"))
        .with_context(|| format!("invalid SOCKS5 proxy {raw:?}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("SOCKS5 proxy authentication is not supported");
    }
    if url.path() != "" && url.path() != "/" {
        bail!("SOCKS5 proxy must not contain a path");
    }
    let host = url
        .host_str()
        .context("SOCKS5 proxy has no host")?
        .to_owned();
    let port = url.port().context("SOCKS5 proxy has no port")?;
    Ok(Endpoint { host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_requested_misspelled_flag() {
        let args = Args::try_parse_from([
            "miner",
            "--startum-url=stratum+tcp://alice:secret@example.com:5575",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.endpoint.host, "example.com");
        assert_eq!(config.endpoint.port, 5575);
        assert_eq!(config.username, "alice");
        assert_eq!(config.password, "secret");
    }

    #[test]
    fn rejects_non_tcp_stratum_scheme() {
        let error = parse_url("stratum+ssl://example.com:443").unwrap_err();
        assert!(error.to_string().contains("unsupported URL scheme"));
    }

    #[test]
    fn parses_socks5_proxy() {
        assert_eq!(
            parse_proxy("127.0.0.1:25344").unwrap(),
            Endpoint {
                host: "127.0.0.1".to_owned(),
                port: 25_344,
            }
        );
        assert!(parse_proxy("user:password@127.0.0.1:25344").is_err());
    }

    #[test]
    fn command_line_selects_gpu_and_batch_size() {
        let args = Args::try_parse_from([
            "miner",
            "--benchmark",
            "--config=missing-test-config.yaml",
            "--device=gpu",
            "--gpu-batch-size=65536",
            "--cuda-kernel=scalar-permute-precompute",
            "--cuda-nonces-per-thread=4",
            "--cuda-block-size=128",
            "--opencl-tuning=retune",
            "--opencl-kernel=scalar-split",
            "--opencl-local-size=128",
            "--opencl-nonces-per-item=2",
            "--benchmark-seconds=30",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.device, DeviceMode::Gpu);
        assert_eq!(config.gpu_batch_size, 65_536);
        assert_eq!(config.cuda_kernel, CudaKernel::ScalarPermutePrecompute);
        assert_eq!(config.cuda_nonces_per_thread, 4);
        assert_eq!(config.cuda_block_size, 128);
        assert_eq!(config.opencl_tuning, OpenClTuning::Retune);
        assert_eq!(config.opencl_kernel, Some(OpenClKernel::ScalarSplit));
        assert_eq!(config.opencl_local_size, Some(128));
        assert_eq!(config.opencl_nonces_per_item, Some(2));
        assert_eq!(config.benchmark_duration, Duration::from_secs(30));
    }

    #[test]
    fn validates_cuda_launch_options() {
        for arguments in [
            ["--cuda-nonces-per-thread=3", "--cuda-block-size=256"],
            ["--cuda-nonces-per-thread=1", "--cuda-block-size=48"],
            ["--cuda-nonces-per-thread=1", "--cuda-block-size=1056"],
        ] {
            let args = Args::try_parse_from([
                "miner",
                "--benchmark",
                "--config=missing-test-config.yaml",
                arguments[0],
                arguments[1],
            ]);
            if let Ok(args) = args {
                assert!(load(args).is_err());
            }
        }
    }

    #[test]
    fn automatic_thread_count_reserves_one_cpu_for_metal() {
        let args = Args::try_parse_from([
            "miner",
            "--benchmark",
            "--config=missing-test-config.yaml",
            "--device=both",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(
            config.threads,
            std::thread::available_parallelism()
                .map_or(1, usize::from)
                .saturating_sub(1)
                .max(1)
        );
    }

    #[test]
    fn explicit_thread_count_is_authoritative() {
        let args = Args::try_parse_from([
            "miner",
            "--benchmark",
            "--config=missing-test-config.yaml",
            "--device=both",
            "--threads=4",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.threads, 4);
    }

    #[test]
    fn parses_gpu_backend_and_device_selection() {
        let args = Args::try_parse_from([
            "miner",
            "--list-devices",
            "--config=missing-test-config.yaml",
            "--gpu-backend=opencl",
            "--gpu-devices=0,2",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.gpu_backend, GpuBackend::Opencl);
        assert_eq!(config.gpu_devices, GpuDevices::Indices(vec![0, 2]));
        assert!(config.list_devices);
    }

    #[test]
    fn file_gpu_devices_accepts_index_list_and_all() {
        let one: FileConfig = serde_yml::from_str("gpu_devices: 2").unwrap();
        let list: FileConfig = serde_yml::from_str("gpu_devices: [0, 3]").unwrap();
        let all: FileConfig = serde_yml::from_str("gpu_devices: all").unwrap();

        assert_eq!(one.gpu_devices, Some(GpuDevices::Indices(vec![2])));
        assert_eq!(list.gpu_devices, Some(GpuDevices::Indices(vec![0, 3])));
        assert_eq!(all.gpu_devices, Some(GpuDevices::All));
    }
}
