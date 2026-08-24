use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{hash::blake2b256, target::Target};

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: String,
    pub blob: Vec<u8>,
    pub target: Target,
    pub network_target: Option<Target>,
    pub extra_nonce2: String,
    pub ntime: String,
}

impl JobSpec {
    pub fn submission(&self, username: &str, request_id: u64, nonce: String) -> Value {
        let params = json!([username, self.id, self.extra_nonce2, self.ntime, nonce]);
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
    pub fn apply_subscribe_response(&mut self, message: &Value) -> Result<()> {
        let result = message
            .get("result")
            .and_then(Value::as_array)
            .context("DATUM subscription response has no result array")?;
        if result.len() < 3 {
            bail!("DATUM subscription result needs extranonce1 and extranonce2 size");
        }
        self.extra_nonce1 = decode_hex(value_string(&result[1], "extranonce1")?)?;
        self.extra_nonce2_size = result[2]
            .as_u64()
            .context("DATUM extranonce2 size is not an integer")?
            as usize;
        if self.extra_nonce2_size != 8 {
            bail!("DATUM BIP-110 requires an 8-byte extranonce2 field");
        }
        Ok(())
    }

    pub fn apply_target(&mut self, method: &str, params: &Value) -> Result<bool> {
        let values = params
            .as_array()
            .context("target notification params are not an array")?;
        let value = values.first().context("target notification has no value")?;
        let target = match method {
            "mining.set_target" => Target::from_hex(value_string(value, "target")?)?,
            "mining.set_difficulty" => Target::from_stratum_difficulty(&difficulty_string(value)?)?,
            _ => return Ok(false),
        };
        self.target = Some(target);
        Ok(true)
    }

    pub fn parse_job(&mut self, params: &Value) -> Result<JobSpec> {
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
        let network_target = Target::from_compact_hex(value_string(&params[6], "nbits")?)
            .context("invalid DATUM network target")?;
        let id = value_string(&params[0], "job ID")?.to_owned();
        let previous = decode_exact(
            value_string(&params[1], "previous ASIC input")?,
            32,
            "DATUM previous ASIC input",
        )?;
        let coinb1_or_mid = decode_hex(value_string(&params[2], "coinb1 or mid")?)?;
        let coinb2 = decode_hex(value_string(&params[3], "coinb2")?)?;
        let branches = params[4]
            .as_array()
            .context("DATUM BIP-110 branch field is not an array")?;
        let ntime = value_string(&params[7], "ntime8")?.to_owned();
        let ntime_bytes = decode_exact(&ntime, 8, "DATUM BIP-110 ntime8")?;

        let (mid, extra_nonce2) =
            if coinb1_or_mid.len() == 32 && coinb2.is_empty() && branches.is_empty() {
                (coinb1_or_mid, "0000000000000000".to_owned())
            } else {
                let extra_nonce2 = self.allocate_extra_nonce2()?;
                let mut arbitrary_transaction = Vec::with_capacity(
                    1 + coinb1_or_mid.len()
                        + self.extra_nonce1.len()
                        + self.extra_nonce2_size
                        + coinb2.len(),
                );
                arbitrary_transaction.push(0);
                arbitrary_transaction.extend_from_slice(&coinb1_or_mid);
                arbitrary_transaction.extend_from_slice(&self.extra_nonce1);
                arbitrary_transaction.extend_from_slice(&decode_hex(&extra_nonce2)?);
                arbitrary_transaction.extend_from_slice(&coinb2);
                let mut merkle_root = blake2b256(&arbitrary_transaction);
                for branch in branches {
                    let branch = decode_exact(
                        value_string(branch, "merkle branch")?,
                        32,
                        "DATUM merkle branch",
                    )?;
                    let mut node = [0u8; 65];
                    node[0] = 1;
                    node[1..33].copy_from_slice(&branch);
                    node[33..].copy_from_slice(&merkle_root);
                    merkle_root = blake2b256(&node);
                }
                (merkle_root.to_vec(), extra_nonce2)
            };

        let mut header = Vec::with_capacity(80);
        header.extend_from_slice(&previous);
        header.extend_from_slice(&[0u8; 8]);
        header.extend_from_slice(&ntime_bytes);
        header.extend_from_slice(&mid);

        Ok(JobSpec {
            id,
            blob: header,
            target,
            network_target: Some(network_target),
            extra_nonce2,
            ntime,
        })
    }

    fn allocate_extra_nonce2(&mut self) -> Result<String> {
        let bits = self.extra_nonce2_size * 8;
        if bits < 64 && self.next_extra_nonce2 >= (1u64 << bits) {
            bail!("DATUM extranonce2 space exhausted");
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
    json!({"id": 1, "method": "mining.subscribe", "params": ["blake2b-miner/0.1.0"]})
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_datum_bip110_header_and_submission() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "01020304", 8]}))
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]))
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
        let job = session.parse_job(&params).unwrap();

        assert_eq!(job.blob.len(), 80);
        assert_eq!(&job.blob[..32], &[0x11; 32]);
        assert_eq!(&job.blob[32..40], &[0; 8]);
        assert_eq!(&job.blob[40..48], hex::decode("0102030405060708").unwrap());
        assert_eq!(&job.blob[48..], &[0x22; 32]);
        assert_eq!(
            job.network_target.as_ref(),
            Some(&Target::from_compact_hex("207fffff").unwrap())
        );
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
    fn builds_datum_profile_zero_from_gateway_fields() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "01020304", 8]}))
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]))
            .unwrap();
        let previous = "11".repeat(32);
        let coinb1 = "22".repeat(39);
        let ntime = "0102030405060708";
        let params = json!([
            "datum-gateway-job",
            previous,
            coinb1,
            "",
            [],
            "30000000",
            "207fffff",
            ntime,
            true
        ]);
        let job = session.parse_job(&params).unwrap();

        let mut arbitrary_transaction = vec![0];
        arbitrary_transaction.extend_from_slice(&[0x22; 39]);
        arbitrary_transaction.extend_from_slice(&[1, 2, 3, 4]);
        arbitrary_transaction.extend_from_slice(&[0; 8]);
        assert_eq!(&job.blob[48..], &blake2b256(&arbitrary_transaction));
        assert_eq!(
            job.submission("local.worker", 10, "8877665544332211".to_owned())["params"],
            json!([
                "local.worker",
                "datum-gateway-job",
                "0000000000000000",
                "0102030405060708",
                "8877665544332211"
            ])
        );
    }

    #[test]
    fn rejects_non_hexadecimal_datum_coinb2() {
        let mut session = SessionState::default();
        session
            .apply_subscribe_response(&json!({"result": [[], "01020304", 8]}))
            .unwrap();
        session
            .apply_target("mining.set_difficulty", &json!([1]))
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

        let error = session.parse_job(&params).unwrap_err();
        assert!(error.to_string().contains("invalid hexadecimal"));
    }
}
