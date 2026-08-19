use std::{fs, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{bail, Context, Result};
use clap::{ArgGroup, Parser, ValueEnum};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ByteOrder {
    Big,
    #[default]
    Little,
}

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

#[derive(Debug, Parser)]
#[command(version, about, group(
  ArgGroup::new("mode").required(true).args(["sia", "datum", "normal"])
))]
pub struct Args {
    /// YAML configuration file.
    #[arg(short, long, default_value = "config.yaml")]
    pub config: PathBuf,

    /// Stratum endpoint. The misspelled --startum-url is retained as an alias.
    #[arg(long, alias = "startum-url")]
    pub stratum_url: Option<String>,

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

    /// Nonces dispatched in each Metal command buffer.
    #[arg(long)]
    pub gpu_batch_size: Option<u32>,

    /// Hash locally instead of connecting to a pool.
    #[arg(long)]
    pub benchmark: bool,

    /// Mine Sia's 80-byte block-header layout and Sia Stratum V1 jobs.
    #[arg(long)]
    pub sia: bool,

    /// Mine the experimental DATUM BIP-110 profile-0 work layout.
    #[arg(long)]
    pub datum: bool,

    /// Mine raw Blake2b-256 blobs using the configured nonce layout.
    #[arg(long)]
    pub normal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sia,
    Datum,
    Normal,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    stratum_url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    threads: Option<usize>,
    device: Option<DeviceMode>,
    gpu_batch_size: Option<u32>,
    nonce_offset: Option<usize>,
    nonce_size: Option<usize>,
    nonce_endian: Option<ByteOrder>,
    hash_byte_order: Option<ByteOrder>,
    reconnect_delay_seconds: Option<u64>,
    stats_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub endpoint: Endpoint,
    pub username: String,
    pub password: String,
    pub threads: usize,
    pub device: DeviceMode,
    pub gpu_batch_size: u32,
    pub nonce_offset: usize,
    pub nonce_size: usize,
    pub nonce_endian: ByteOrder,
    pub hash_byte_order: ByteOrder,
    pub reconnect_delay: Duration,
    pub stats_interval: Duration,
    pub benchmark: bool,
    pub mode: Mode,
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
    } else if args.stratum_url.is_some() || args.benchmark {
        FileConfig::default()
    } else {
        bail!(
            "configuration file {} does not exist",
            args.config.display()
        );
    };

    let raw_url = args
        .stratum_url
        .or(file.stratum_url)
        .unwrap_or_else(|| "stratum+tcp://127.0.0.1:3333".to_owned());
    let url = parse_url(&raw_url)?;
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
    let threads = args.threads.or(file.threads).unwrap_or(0);
    let threads = if threads == 0 {
        std::thread::available_parallelism().map_or(1, usize::from)
    } else {
        threads
    };
    let nonce_size = file.nonce_size.unwrap_or(8);
    if !(1..=8).contains(&nonce_size) {
        bail!("nonce_size must be between 1 and 8 bytes");
    }
    let gpu_batch_size = args
        .gpu_batch_size
        .or(file.gpu_batch_size)
        .unwrap_or(1_048_576);
    if gpu_batch_size == 0 {
        bail!("gpu_batch_size must be greater than zero");
    }

    Ok(Config {
        endpoint: url.endpoint,
        username,
        password,
        threads,
        device: args.device.or(file.device).unwrap_or_default(),
        gpu_batch_size,
        nonce_offset: file.nonce_offset.unwrap_or(32),
        nonce_size,
        nonce_endian: file.nonce_endian.unwrap_or_default(),
        hash_byte_order: file.hash_byte_order.unwrap_or_default(),
        reconnect_delay: Duration::from_secs(file.reconnect_delay_seconds.unwrap_or(5)),
        stats_interval: Duration::from_secs(file.stats_interval_seconds.unwrap_or(5).max(1)),
        benchmark: args.benchmark,
        mode: if args.sia {
            Mode::Sia
        } else if args.datum {
            Mode::Datum
        } else {
            Mode::Normal
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_requested_misspelled_flag() {
        let args = Args::try_parse_from([
            "miner",
            "--normal",
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
    fn command_line_selects_gpu_and_batch_size() {
        let args = Args::try_parse_from([
            "miner",
            "--sia",
            "--benchmark",
            "--config=missing-test-config.yaml",
            "--device=gpu",
            "--gpu-batch-size=65536",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.device, DeviceMode::Gpu);
        assert_eq!(config.gpu_batch_size, 65_536);
    }

    #[test]
    fn command_line_selects_datum_mode() {
        let args = Args::try_parse_from([
            "miner",
            "--datum",
            "--benchmark",
            "--config=missing-test-config.yaml",
        ])
        .unwrap();
        let config = load(args).unwrap();

        assert_eq!(config.mode, Mode::Datum);
    }

    #[test]
    fn protocol_modes_are_mutually_exclusive() {
        assert!(Args::try_parse_from(["miner", "--sia", "--datum"]).is_err());
    }
}
