use std::any::Any;

use anyhow::Result;

use crate::protocol::JobSpec;

mod metal;

/// Identity shared by every GPU backend.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub backend: &'static str,
    pub index: usize,
    pub name: String,
}

/// A job in the backend's native representation.
pub trait PreparedJob: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Send + Sync> PreparedJob for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Synchronous GPU mining boundary.
///
/// Backends own their device resources and translate each DATUM job into their
/// preferred representation. Candidate nonces are always verified by Rust
/// before submission.
pub trait Backend: Send {
    fn device_info(&self) -> &DeviceInfo;
    fn batch_size(&self) -> u64;
    fn prepare_job(&self, spec: &JobSpec) -> Result<Box<dyn PreparedJob>>;
    fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>>;
}

pub fn default_backend(batch_size: u32) -> Result<Box<dyn Backend>> {
    Ok(Box::new(metal::MetalBackend::new(batch_size)?))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;
    use crate::{protocol::JobSpec, target::Target};

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn metal_matches_reference_for_datum_layout() {
        let Ok(mut backend) = default_backend(1_024) else {
            eprintln!("skipping Metal test because no GPU is exposed");
            return;
        };
        let blob = vec![0x5a; 80];
        let start_nonce = u32::MAX as u64 - 511;
        let mut hashes = (0..backend.batch_size())
            .map(|offset| {
                let nonce = start_nonce + offset;
                (nonce, reference_hash(&blob, nonce))
            })
            .collect::<Vec<_>>();
        hashes.sort_unstable_by_key(|(_, hash)| *hash);
        let selected = hashes[31].1;
        let target = Target::from_hex(&hex::encode(selected)).unwrap();
        let spec = JobSpec {
            id: "gpu-test".to_owned(),
            blob,
            target: target.clone(),
            extra_nonce2: "0000000000000000".to_owned(),
            ntime: "0000000000000000".to_owned(),
        };
        let mut expected = hashes
            .iter()
            .filter_map(|(nonce, hash)| target.accepts(hash).then_some(*nonce))
            .collect::<Vec<_>>();
        let job = backend.prepare_job(&spec).unwrap();
        let mut actual = backend.mine(job.as_ref(), start_nonce).unwrap();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    fn reference_hash(blob: &[u8], nonce: u64) -> [u8; 32] {
        let mut input = blob.to_vec();
        input[32..40].copy_from_slice(&nonce.to_le_bytes());
        ReferenceBlake2b256::digest(input).into()
    }
}
