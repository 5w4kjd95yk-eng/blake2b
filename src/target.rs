use std::{cmp::Ordering, fmt};

use anyhow::{bail, Context, Result};
use num_bigint::BigUint;
use num_traits::Zero;

use crate::config::ByteOrder;

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
        let difficulty_one = BigUint::parse_bytes(
            b"00000000ffff0000000000000000000000000000000000000000000000000000",
            16,
        )
        .unwrap();
        let target = difficulty_one * denominator / numerator;
        let bytes = target.to_bytes_be();
        if bytes.len() > 32 {
            return Ok(Self([0xff; 32]));
        }
        let mut output = [0u8; 32];
        output[32 - bytes.len()..].copy_from_slice(&bytes);
        Ok(Self(output))
    }

    #[inline]
    pub fn accepts(&self, digest: &[u8; 32], order: ByteOrder) -> bool {
        let ordering = match order {
            ByteOrder::Big => digest.as_slice().cmp(&self.0),
            ByteOrder::Little => digest.iter().rev().cmp(self.0.iter()),
        };
        ordering != Ordering::Greater
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_sia_stratum_difficulty_one_target() {
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
    fn compares_hashes_in_both_orders() {
        let target = Target::from_hex("01ff").unwrap();
        let mut big_hash = [0u8; 32];
        big_hash[30..].copy_from_slice(&[1, 0xfe]);
        assert!(target.accepts(&big_hash, ByteOrder::Big));

        let mut little_hash = [0u8; 32];
        little_hash[..2].copy_from_slice(&[0xfe, 1]);
        assert!(target.accepts(&little_hash, ByteOrder::Little));
    }
}
