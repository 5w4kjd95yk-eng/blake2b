use std::{cmp::Ordering, fmt};

use anyhow::{bail, Context, Result};
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};

#[derive(Clone, PartialEq, Eq)]
pub struct Target([u8; 32]);

impl Target {
    pub fn from_hex(value: &str) -> Result<Self> {
        let value = value.strip_prefix("0x").unwrap_or(value);
        if value.is_empty() || value.len() > 64 {
            bail!("target must contain 1 to 64 hexadecimal characters");
        }
        let normalized = if value.len() % 2 == 1 {
            format!("0{value}")
        } else {
            value.to_owned()
        };
        let bytes = hex::decode(&normalized).context("target is not hexadecimal")?;
        let mut target = [0u8; 32];
        target[32 - bytes.len()..].copy_from_slice(&bytes);
        Ok(Self(target))
    }

    pub fn from_stratum_difficulty(value: &str) -> Result<Self> {
        let (numerator, denominator) = decimal_ratio(value)?;
        if numerator.is_zero() {
            bail!("Stratum difficulty must be greater than zero");
        }
        let target = difficulty_one() * denominator / numerator;
        Ok(Self::from_biguint_saturating(target))
    }

    pub fn from_compact_hex(value: &str) -> Result<Self> {
        let value = value.strip_prefix("0x").unwrap_or(value);
        if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("compact target must contain exactly 8 hexadecimal characters");
        }
        let compact =
            u32::from_str_radix(value, 16).context("compact target is not hexadecimal")?;
        let exponent = compact >> 24;
        let mantissa = compact & 0x007f_ffff;
        if mantissa == 0 {
            bail!("compact target is zero");
        }
        if compact & 0x0080_0000 != 0 {
            bail!("compact target is negative");
        }

        let target = if exponent <= 3 {
            BigUint::from(mantissa >> (8 * (3 - exponent)))
        } else {
            BigUint::from(mantissa) << (8 * (exponent - 3))
        };
        if target.is_zero() {
            bail!("compact target is zero");
        }
        let bytes = target.to_bytes_be();
        if bytes.len() > 32 {
            bail!("compact target exceeds 256 bits");
        }
        let mut output = [0u8; 32];
        output[32 - bytes.len()..].copy_from_slice(&bytes);
        Ok(Self(output))
    }

    #[inline]
    pub fn accepts(&self, digest: &[u8; 32]) -> bool {
        digest.as_slice().cmp(&self.0) != Ordering::Greater
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn difficulty_for_hash(digest: &[u8; 32]) -> f64 {
        difficulty_one_as_f64()
            / BigUint::from_bytes_be(digest)
                .to_f64()
                .unwrap_or(f64::INFINITY)
    }

    pub fn difficulty(&self) -> f64 {
        difficulty_one_as_f64()
            / BigUint::from_bytes_be(&self.0)
                .to_f64()
                .unwrap_or(f64::INFINITY)
    }

    pub fn words_be(&self) -> [u64; 4] {
        let mut words = [0u64; 4];
        for (word, bytes) in words.iter_mut().zip(self.0.chunks_exact(8)) {
            *word = u64::from_be_bytes(bytes.try_into().unwrap());
        }
        words
    }

    fn from_biguint_saturating(value: BigUint) -> Self {
        let bytes = value.to_bytes_be();
        if bytes.len() > 32 {
            return Self([0xff; 32]);
        }
        let mut output = [0u8; 32];
        output[32 - bytes.len()..].copy_from_slice(&bytes);
        Self(output)
    }
}

impl fmt::Debug for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Target")
            .field(&self.as_hex())
            .finish()
    }
}

fn decimal_ratio(value: &str) -> Result<(BigUint, BigUint)> {
    let value = value.trim();
    if value.starts_with('-') || value.is_empty() {
        bail!("invalid Stratum difficulty {value:?}");
    }
    let (mantissa, exponent) = match value.find(['e', 'E']) {
        Some(index) => (&value[..index], value[index + 1..].parse::<i32>()?),
        None => (value, 0),
    };
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fractional.is_empty() {
        bail!("invalid Stratum difficulty {value:?}");
    }
    let digits = format!("{whole}{fractional}");
    let mut numerator = BigUint::parse_bytes(digits.as_bytes(), 10)
        .with_context(|| format!("invalid Stratum difficulty {value:?}"))?;
    let scale = fractional.len() as i32 - exponent;
    let mut denominator = BigUint::from(1u8);
    if scale >= 0 {
        denominator = BigUint::from(10u8).pow(scale as u32);
    } else {
        numerator *= BigUint::from(10u8).pow((-scale) as u32);
    }
    Ok((numerator, denominator))
}

fn difficulty_one() -> BigUint {
    BigUint::parse_bytes(
        b"00000000ffff0000000000000000000000000000000000000000000000000000",
        16,
    )
    .unwrap()
}

fn difficulty_one_as_f64() -> f64 {
    difficulty_one().to_f64().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_stratum_difficulty_one_target() {
        let target = Target::from_stratum_difficulty("1").unwrap();
        assert_eq!(
            target.as_hex(),
            "00000000ffff0000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn handles_decimal_and_exponent_difficulty() {
        assert_eq!(
            Target::from_stratum_difficulty("2.5").unwrap(),
            Target::from_stratum_difficulty("25e-1").unwrap()
        );
    }

    #[test]
    fn decodes_bitcoin_compact_targets() {
        assert_eq!(
            Target::from_compact_hex("1d00ffff").unwrap(),
            Target::from_stratum_difficulty("1").unwrap()
        );
        assert_eq!(
            Target::from_compact_hex("207fffff").unwrap().as_hex(),
            format!("7fffff{}", "00".repeat(29))
        );
    }

    #[test]
    fn rejects_invalid_compact_targets() {
        assert!(Target::from_compact_hex("1d80ffff").is_err());
        assert!(Target::from_compact_hex("21010000").is_err());
        assert!(Target::from_compact_hex("00000000").is_err());
        assert!(Target::from_compact_hex("01000001").is_err());
    }

    #[test]
    fn calculates_actual_hash_difficulty() {
        let difficulty_one = Target::from_stratum_difficulty("1").unwrap();
        let digest: [u8; 32] = hex::decode(difficulty_one.as_hex())
            .unwrap()
            .try_into()
            .unwrap();

        assert!((Target::difficulty_for_hash(&digest) - 1.0).abs() < 1e-12);
        assert!((difficulty_one.difficulty() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn compares_big_endian_hashes() {
        let target = Target::from_hex("01ff").unwrap();
        let mut big_hash = [0u8; 32];
        big_hash[30..].copy_from_slice(&[1, 0xfe]);
        assert!(target.accepts(&big_hash));
    }
}
