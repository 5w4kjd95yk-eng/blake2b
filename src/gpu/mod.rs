use std::any::Any;

use anyhow::{bail, Result};

use crate::{
    config::{GpuBackend, GpuDevices},
    protocol::JobSpec,
};

mod cuda;
mod metal;
mod opencl;

/// Identity shared by every GPU backend.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub backend: &'static str,
    pub index: usize,
    pub name: String,
    pub compute_capability: Option<(u32, u32)>,
    pub total_memory: Option<u64>,
    pub usable_memory: Option<u64>,
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
    fn prepare_job(&self, spec: &JobSpec, generation: u64) -> Result<Box<dyn PreparedJob>>;
    fn mine(&mut self, job: &dyn PreparedJob, start_nonce: u64) -> Result<Vec<u64>>;
}

pub fn devices(requested: GpuBackend) -> Result<Vec<DeviceInfo>> {
    match resolve_backend(requested)? {
        GpuBackend::Metal => {
            let backend = metal::MetalBackend::new(1)?;
            Ok(vec![backend.device_info().clone()])
        }
        GpuBackend::Cuda => cuda::devices(),
        GpuBackend::Opencl => opencl::devices(),
        GpuBackend::Auto => unreachable!(),
    }
}

pub fn backends(
    requested: GpuBackend,
    selected: &GpuDevices,
    batch_size: u32,
) -> Result<Vec<Box<dyn Backend>>> {
    match resolve_backend(requested)? {
        GpuBackend::Metal => {
            let indices = selected_indices(selected, 1)?;
            if indices != [0] {
                bail!("Metal exposes only GPU device index 0");
            }
            Ok(vec![Box::new(metal::MetalBackend::new(batch_size)?)])
        }
        GpuBackend::Cuda => {
            let available = cuda::devices()?;
            selected_indices(selected, available.len())?
                .into_iter()
                .map(|index| cuda::backend(index, batch_size))
                .collect()
        }
        GpuBackend::Opencl => {
            let available = opencl::devices()?;
            selected_indices(selected, available.len())?
                .into_iter()
                .map(|index| opencl::backend(index, batch_size))
                .collect()
        }
        GpuBackend::Auto => unreachable!(),
    }
}

fn resolve_backend(requested: GpuBackend) -> Result<GpuBackend> {
    match requested {
        GpuBackend::Auto if cfg!(target_os = "macos") => Ok(GpuBackend::Metal),
        GpuBackend::Auto if cfg!(all(target_os = "linux", feature = "cuda")) => {
            if cuda::devices()?.is_empty() {
                bail!("no NVIDIA CUDA devices are available");
            }
            Ok(GpuBackend::Cuda)
        }
        GpuBackend::Auto => bail!(
            "no automatic GPU backend is available on this build; choose CPU or rebuild with CUDA"
        ),
        backend => Ok(backend),
    }
}

fn selected_indices(selected: &GpuDevices, count: usize) -> Result<Vec<usize>> {
    let indices = match selected {
        GpuDevices::All => (0..count).collect(),
        GpuDevices::Indices(indices) => indices.clone(),
    };
    if indices.is_empty() {
        bail!("no GPU devices are available");
    }
    if let Some(index) = indices.iter().find(|index| **index >= count) {
        bail!("GPU device index {index} does not exist (found {count} devices)");
    }
    Ok(indices)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use blake2::{digest::consts::U32, Blake2b, Digest};

    use super::*;
    use crate::{protocol::JobSpec, target::Target};

    type ReferenceBlake2b256 = Blake2b<U32>;

    #[test]
    fn metal_matches_reference_for_datum_layout() {
        let Ok(mut backend) = backends(GpuBackend::Metal, &GpuDevices::default(), 1_024)
            .map(|mut backends| backends.remove(0))
        else {
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
        let job = backend.prepare_job(&spec, 1).unwrap();
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
