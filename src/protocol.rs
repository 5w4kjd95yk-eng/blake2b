use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{
    config::{ByteOrder, Config, Mode},
    hash::blake2b256,
    target::Target,
};

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: String,
    pub blob: Vec<u8>,
    pub target: Target,
    pub nonce_offset: usize,
    pub nonce_size: usize,
    pub nonce_order: ByteOrder,
    pub hash_order: ByteOrder,
    pub submit: Submit,
}

#[derive(Clone, Debug)]
pub enum Submit {
    Sia { extra_nonce2: String, ntime: String },
    Datum { ntime: String },
    Normal,
}

impl JobSpec {
    pub fn submission(&self, username: &str, request_id: u64, nonce: String) -> Value {
        let params = match &self.submit {
            Submit::Sia {
                extra_nonce2,
                ntime,
            } => json!([username, self.id, extra_nonce2, ntime, nonce]),
            Submit::Datum { ntime } => {
                json!([username, self.id, "0000000000000000", ntime, nonce])
            }
            Submit::Normal => json!([username, self.id, nonce]),
        };
        json!({"id": request_id, "method": "mining.submit", "params": params})
    }
}

#[derive(Debug, Default)]
pub struct SessionState {
    pub target: Option<Target>,
    extra_nonce1: Vec<u8>,
    extra_nonce2_size: usize,
    next_extra_nonce2: u64,
}

impl SessionState {
    pub fn apply_subscribe_response(&mut self, message: &Value, mode: Mode) -> Result<()> {
        if !matches!(mode, Mode::Sia | Mode::Datum) {
            return Ok(());
        }
        let protocol = if mode == Mode::Datum { "DATUM" } else { "Sia" };
        let result = message
            .get("result")
            .and_then(Value::as_array)
            .with_context(|| format!("{protocol} subscription response has no result array"))?;
        if result.len() < 3 {
            bail!("{protocol} subscription result needs extranonce1 and extranonce2 size");
        }
        self.extra_nonce1 = decode_hex(value_string(&result[1], "extranonce1")?)?;
        self.extra_nonce2_size = result[2]
            .as_u64()
            .with_context(|| format!("{protocol} extranonce2 size is not an integer"))?
            as usize;
        if self.extra_nonce2_size == 0 || self.extra_nonce2_size > 8 {
            bail!("{protocol} extranonce2 size must be between 1 and 8 bytes");
        }
        if mode == Mode::Datum && self.extra_nonce2_size != 8 {
            bail!("DATUM BIP-110 requires an 8-byte extranonce2 field");
        }
        Ok(())
    }

    pub fn apply_target(&mut self, method: &str, params: &Value, mode: Mode) -> Result<bool> {
        let values = params
            .as_array()
            .context("target notification params are not an array")?;
        let value = values.first().context("target notification has no value")?;
        let target = match method {
            "mining.set_target" => Target::from_hex(value_string(value, "target")?)?,
            "mining.set_difficulty" if matches!(mode, Mode::Sia | Mode::Datum) => {
                Target::from_stratum_difficulty(&difficulty_string(value)?)?
            }
            "mining.set_difficulty" => return Ok(false),
            _ => return Ok(false),
        };
        self.target = Some(target);
        Ok(true)
    }

    pub fn parse_job(&mut self, params: &Value, config: &Config) -> Result<JobSpec> {
        match config.mode {
            Mode::Sia => self.parse_sia_job(params),
            Mode::Datum => self.parse_datum_job(params),
            Mode::Normal => self.parse_normal_job(params, config),
        }
    }

    fn parse_sia_job(&mut self, params: &Value) -> Result<JobSpec> {
        let params = params
            .as_array()
            .context("Sia mining.notify params are not an array")?;
        if params.len() != 9 {
            bail!(
                "Sia mining.notify requires exactly 9 parameters, got {}",
                params.len()
            );
        }
        if self.extra_nonce2_size == 0 {
            bail!("Sia job arrived before a valid subscription response");
        }
        let target = self
            .target
            .clone()
            .context("Sia job arrived before mining.set_difficulty or mining.set_target")?;
        let id = value_string(&params[0], "job ID")?.to_owned();
        let parent = decode_exact(value_string(&params[1], "prevhash")?, 32, "Sia prevhash")?;
        let coinb1 = decode_hex(value_string(&params[2], "coinb1")?)?;
        let coinb2 = decode_hex(value_string(&params[3], "coinb2")?)?;
        let branches = params[4]
            .as_array()
            .context("Sia merkle branches are not an array")?;
        let ntime = value_string(&params[7], "ntime")?.to_owned();
        let ntime_bytes = decode_exact(&ntime, 8, "Sia ntime")?;
        let extra_nonce2 = self.allocate_extra_nonce2()?;

        let mut arbitrary_transaction = Vec::with_capacity(
            1 + coinb1.len() + self.extra_nonce1.len() + self.extra_nonce2_size + coinb2.len(),
        );
        arbitrary_transaction.push(0);
        arbitrary_transaction.extend_from_slice(&coinb1);
        arbitrary_transaction.extend_from_slice(&self.extra_nonce1);
        arbitrary_transaction.extend_from_slice(&decode_hex(&extra_nonce2)?);
        arbitrary_transaction.extend_from_slice(&coinb2);
        let mut merkle_root = blake2b256(&arbitrary_transaction);
        for branch in branches {
            let branch = decode_exact(
                value_string(branch, "merkle branch")?,
                32,
                "Sia merkle branch",
            )?;
            let mut node = [0u8; 65];
            node[0] = 1;
            node[1..33].copy_from_slice(&branch);
            node[33..].copy_from_slice(&merkle_root);
            merkle_root = blake2b256(&node);
        }

        let mut header = Vec::with_capacity(80);
        header.extend_from_slice(&parent);
        header.extend_from_slice(&[0u8; 8]);
        header.extend_from_slice(&ntime_bytes);
        header.extend_from_slice(&merkle_root);

        Ok(JobSpec {
            id,
            blob: header,
            target,
            nonce_offset: 32,
            nonce_size: 8,
            nonce_order: ByteOrder::Little,
            hash_order: ByteOrder::Big,
            submit: Submit::Sia {
                extra_nonce2,
                ntime,
            },
        })
    }

    fn parse_datum_job(&self, params: &Value) -> Result<JobSpec> {
        let params = params
            .as_array()
            .context("DATUM mining.notify params are not an array")?;
        if params.len() != 9 {
            bail!(
                "DATUM mining.notify requires exactly 9 parameters, got {}",
                params.len()
            );
        }
        if self.extra_nonce2_size != 8 {
            bail!("DATUM job arrived before a valid subscription response");
        }
        let target = self
            .target
            .clone()
            .context("DATUM job arrived before mining.set_difficulty or mining.set_target")?;
        let id = value_string(&params[0], "job ID")?.to_owned();
        let previous = decode_exact(
            value_string(&params[1], "previous ASIC input")?,
            32,
            "DATUM previous ASIC input",
        )?;
        let mid = decode_exact(value_string(&params[2], "mid")?, 32, "DATUM BIP-110 mid")?;
        if !value_string(&params[3], "reserved coinb2")?.is_empty() {
            bail!("DATUM BIP-110 coinb2 field must be empty");
        }
        let branches = params[4]
            .as_array()
            .context("DATUM BIP-110 branch field is not an array")?;
        if !branches.is_empty() {
            bail!("DATUM BIP-110 branch field must be empty");
        }
        let ntime = value_string(&params[7], "ntime8")?.to_owned();
        let ntime_bytes = decode_exact(&ntime, 8, "DATUM BIP-110 ntime8")?;

        let mut header = Vec::with_capacity(80);
        header.extend_from_slice(&previous);
        header.extend_from_slice(&[0u8; 8]);
        header.extend_from_slice(&ntime_bytes);
        header.extend_from_slice(&mid);

        Ok(JobSpec {
            id,
            blob: header,
            target,
            nonce_offset: 32,
            nonce_size: 8,
            nonce_order: ByteOrder::Little,
            hash_order: ByteOrder::Big,
            submit: Submit::Datum { ntime },
        })
    }

    fn parse_normal_job(&self, params: &Value, config: &Config) -> Result<JobSpec> {
        let (id, blob, target, nonce_offset, nonce_size, nonce_order, hash_order) =
            if let Some(object) = params.as_object() {
                let id = object
                    .get("job_id")
                    .or_else(|| object.get("id"))
                    .context("raw job has no job_id")?;
                let blob = object
                    .get("blob")
                    .or_else(|| object.get("data"))
                    .or_else(|| object.get("header"))
                    .context("raw job has no blob")?;
                let target = match object.get("target") {
                    Some(value) => Target::from_hex(value_string(value, "target")?)?,
                    None => self.target.clone().context("raw job has no target")?,
                };
                let nonce_offset = optional_usize(object.get("nonce_offset"), config.nonce_offset)?;
                let nonce_size = optional_usize(object.get("nonce_size"), config.nonce_size)?;
                let nonce_order =
                    optional_byte_order(object.get("nonce_endian"), config.nonce_endian)?;
                let hash_order =
                    optional_byte_order(object.get("hash_byte_order"), config.hash_byte_order)?;
                (
                    value_string(id, "job ID")?.to_owned(),
                    value_string(blob, "blob")?.to_owned(),
                    target,
                    nonce_offset,
                    nonce_size,
                    nonce_order,
                    hash_order,
                )
            } else {
                let values = params
                    .as_array()
                    .context("raw mining.notify params are not an array")?;
                if values.len() < 2 || values.len() > 4 {
                    bail!("raw mining.notify expects [job_id, blob, target?, clean_jobs?]");
                }
                let target = if values.get(2).is_some_and(Value::is_string) {
                    Target::from_hex(value_string(&values[2], "target")?)?
                } else {
                    self.target.clone().context("raw job has no target")?
                };
                (
                    value_string(&values[0], "job ID")?.to_owned(),
                    value_string(&values[1], "blob")?.to_owned(),
                    target,
                    config.nonce_offset,
                    config.nonce_size,
                    config.nonce_endian,
                    config.hash_byte_order,
                )
            };

        if !(1..=8).contains(&nonce_size) {
            bail!("raw job nonce_size must be between 1 and 8 bytes");
        }
        let blob = decode_hex(&blob)?;
        let nonce_end = nonce_offset
            .checked_add(nonce_size)
            .context("raw job nonce range overflows")?;
        if nonce_end > blob.len() {
            bail!(
                "raw job nonce range {nonce_offset}..{nonce_end} exceeds {}-byte blob",
                blob.len()
            );
        }
        if blob.len() > 128 {
            bail!("raw jobs longer than one 128-byte Blake2b block are not supported by the SIMD miner");
        }

        Ok(JobSpec {
            id,
            blob,
            target,
            nonce_offset,
            nonce_size,
            nonce_order,
            hash_order,
            submit: Submit::Normal,
        })
    }

    fn allocate_extra_nonce2(&mut self) -> Result<String> {
        let bits = self.extra_nonce2_size * 8;
        if bits < 64 && self.next_extra_nonce2 >= (1u64 << bits) {
            bail!("Sia extranonce2 space exhausted");
        }
        let value = self.next_extra_nonce2;
        self.next_extra_nonce2 = self.next_extra_nonce2.wrapping_add(1);
        Ok(format!(
            "{value:0width$x}",
            width = self.extra_nonce2_size * 2
        ))
    }
}

pub fn subscribe_request() -> Value {
    json!({"id": 1, "method": "mining.subscribe", "params": ["blake2b-apple-miner/0.1.0"]})
}

pub fn authorize_request(username: &str, password: &str) -> Value {
    json!({"id": 2, "method": "mining.authorize", "params": [username, password]})
}

fn value_string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .as_str()
        .with_context(|| format!("{name} is not a string"))
}

fn difficulty_string(value: &Value) -> Result<String> {
    match value {
        Value::Number(number) => Ok(number.to_string()),
        Value::String(string) => Ok(string.clone()),
        _ => bail!("difficulty is not a number or decimal string"),
    }
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    hex::decode(value).with_context(|| format!("invalid hexadecimal value {value:?}"))
}

fn decode_exact(value: &str, length: usize, name: &str) -> Result<Vec<u8>> {
    let bytes = decode_hex(value)?;
    if bytes.len() != length {
        bail!("{name} must be {length} bytes, got {}", bytes.len());
    }
    Ok(bytes)
}

fn optional_usize(value: Option<&Value>, default: usize) -> Result<usize> {
    match value {
        Some(value) => Ok(value.as_u64().context("job integer field is invalid")? as usize),
        None => Ok(default),
    }
}

fn optional_byte_order(value: Option<&Value>, default: ByteOrder) -> Result<ByteOrder> {
    match value {
        Some(value) => serde_json::from_value(value.clone()).context("invalid byte order"),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::{DeviceMode, Endpoint};

    fn config(mode: Mode) -> Config {
        Config {
            endpoint: Endpoint {
                host: "localhost".to_owned(),
                port: 3333,
            },
            username: "worker".to_owned(),
            password: "x".to_owned(),
            threads: 1,
            device: DeviceMode::Cpu,
            gpu_batch_size: 1024,
            nonce_offset: 4,
            nonce_size: 8,
            nonce_endian: ByteOrder::Little,
            hash_byte_order: ByteOrder::Big,
            reconnect_delay: Duration::from_secs(1),
            stats_interval: Duration::from_secs(1),
            benchmark: false,
            mode,
        }
    }

    #[test]
    fn builds_sia_header_and_submission_fields() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "aabb", 4]}), Mode::Sia)
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]), Mode::Sia)
            .unwrap();
        let params = json!([
            "bf",
            "00".repeat(32),
            "11",
            "22",
            ["33".repeat(32)],
            "",
            "1a08645a",
            "58258e5700000000",
            false
        ]);
        let job = session.parse_job(&params, &config(Mode::Sia)).unwrap();

        assert_eq!(job.blob.len(), 80);
        assert_eq!(&job.blob[..32], &[0u8; 32]);
        assert_eq!(&job.blob[32..40], &[0u8; 8]);
        assert_eq!(&job.blob[40..48], &hex::decode("58258e5700000000").unwrap());
        assert_eq!(job.nonce_offset, 32);
        assert_eq!(job.hash_order, ByteOrder::Big);
        assert_eq!(
            job.submission("worker", 4, "b2957c0000000000".to_owned())["params"],
            json!([
                "worker",
                "bf",
                "00000000",
                "58258e5700000000",
                "b2957c0000000000"
            ])
        );
    }

    #[test]
    fn parses_normal_object_job_overrides() {
        let mut session = SessionState::default();
        let params = json!({
          "job_id": "raw-1",
          "blob": "00".repeat(24),
          "target": "00ff",
          "nonce_offset": 8,
          "nonce_size": 4,
          "nonce_endian": "big",
          "hash_byte_order": "little"
        });
        let job = session.parse_job(&params, &config(Mode::Normal)).unwrap();

        assert_eq!(job.id, "raw-1");
        assert_eq!(job.nonce_offset, 8);
        assert_eq!(job.nonce_size, 4);
        assert_eq!(job.nonce_order, ByteOrder::Big);
        assert_eq!(job.hash_order, ByteOrder::Little);
    }

    #[test]
    fn builds_datum_bip110_header_and_submission() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "01020304", 8]}), Mode::Datum)
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]), Mode::Datum)
            .unwrap();
        let previous = "11".repeat(32);
        let mid = "22".repeat(32);
        let ntime = "0102030405060708";
        let params = json!([
            "datum-job",
            previous,
            mid,
            "",
            [],
            "20000000",
            "207fffff",
            ntime,
            true
        ]);
        let job = session.parse_job(&params, &config(Mode::Datum)).unwrap();

        assert_eq!(job.blob.len(), 80);
        assert_eq!(&job.blob[..32], &[0x11; 32]);
        assert_eq!(&job.blob[32..40], &[0; 8]);
        assert_eq!(&job.blob[40..48], hex::decode("0102030405060708").unwrap());
        assert_eq!(&job.blob[48..], &[0x22; 32]);
        assert_eq!(job.nonce_offset, 32);
        assert_eq!(job.nonce_size, 8);
        assert_eq!(job.nonce_order, ByteOrder::Little);
        assert_eq!(job.hash_order, ByteOrder::Big);
        assert_eq!(
            job.submission("local.worker", 10, "8877665544332211".to_owned())["params"],
            json!([
                "local.worker",
                "datum-job",
                "0000000000000000",
                "0102030405060708",
                "8877665544332211"
            ])
        );
    }

    #[test]
    fn rejects_non_profile_zero_datum_package() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "01020304", 8]}), Mode::Datum)
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]), Mode::Datum)
            .unwrap();
        let params = json!([
            "datum-job",
            "11".repeat(32),
            "22".repeat(32),
            "unexpected",
            [],
            "20000000",
            "207fffff",
            "00".repeat(8),
            true
        ]);

        let error = session
            .parse_job(&params, &config(Mode::Datum))
            .unwrap_err();
        assert!(error.to_string().contains("coinb2 field must be empty"));
    }
}
