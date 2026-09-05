# blake2b-miner

DATUM BIP-110 profile-0 Blake2b-256 Stratum miner. On Apple silicon, the CPU
hot loop hashes four independent nonces at once using AArch64 NEON and the
default GPU backend runs a Metal compute kernel with four nonces per thread.
CPU and GPU workers reserve disjoint nonce ranges from the same job.

The GPU worker uses a backend-neutral synchronous interface for device
identity, job preparation, batch dispatch, and candidate collection. Metal,
CUDA, and OpenCL implementations are available. CUDA and OpenCL are opt-in and
are not required by default builds. The OpenCL implementation is a portable,
auto-tuned backend whose split 64-bit rotation core is adapted from the tuned
[sgminer-blake2b Sia kernel](https://github.com/zhq1/sgminer-blake2b/blob/master/kernel/sia.cl).

Rust fits this job. It exposes AArch64 intrinsics without requiring assembly,
keeps the networking/configuration code memory-safe, and has no runtime or GC
in the hash loop.

## Build

```sh
cargo build --release
```

This automatically includes Metal on macOS, OpenCL through its dynamic runtime
loader, and CUDA on native Linux builds when a CUDA toolkit containing `nvcc`
is found. CPU mining is always available. GPU devices and drivers are detected
when the miner starts.

Use the `cuda` feature to require CUDA support instead of silently omitting it
when the toolkit is absent or unusable:

```sh
cargo build --release --features cuda
```

`CUDA_PATH` (or `CUDA_HOME`) overrides the toolkit root. Fleet builds can set
`CUDA_ARCHITECTURES` to a comma-separated compute capability list, for example
`75,86,89,120`. The default is `75,80,86,89,120`.

The CUDA backend defaults to the scalar DATUM kernel with one nonce per thread
and 512 threads per block. The original reference kernel and other tuning
variants remain selectable for correctness checks and controlled benchmarking:

```sh
target/release/blake2b-miner \
  --benchmark --device=gpu --gpu-backend=cuda \
  --cuda-kernel=scalar-permute-precompute \
  --cuda-nonces-per-thread=2 --cuda-block-size=128
```

`--cuda-kernel` accepts `reference`, `scalar`, `scalar-permute`, and
`scalar-permute-precompute`. Nonces per thread may be 1, 2, or 4; block size
must be a multiple of 32 from 32 through 1024. The same settings may be placed
in YAML as `cuda_kernel`, `cuda_nonces_per_thread`, and `cuda_block_size`.
The selected kernel and launch geometry are printed when the CUDA backend is
created. These overrides do not affect Metal or OpenCL.

The Metal backend validates and profiles DATUM kernel, nonce-width, and
threadgroup candidates on first use. Its selection is cached in
`~/Library/Caches/blake2b-miner/metal-tuning.json`. Use
`--metal-tuning=retune` after a hardware or operating-system change, or
`--metal-tuning=off` for the legacy array/split kernel with four nonces per
thread and a 64-thread group. `--metal-kernel`,
`--metal-nonces-per-thread`, and `--metal-threadgroup-size` constrain the
candidate set for diagnostics.

`BLAKE2B_CUDA=off`, `auto`, or `force` explicitly controls CUDA detection and
overrides the feature-derived mode. Automatic CUDA detection is disabled while
cross-compiling. Use `--no-default-features` for a CPU-only build.

OpenCL is included by default and loads the system implementation at runtime.
It can also be selected explicitly when default features are disabled:

```sh
cargo build --release --features opencl
```

The DATUM OpenCL backend validates and briefly profiles portable OpenCL C 1.2
kernel and work-group candidates the first time it sees a device/driver/kernel
combination. The winner is cached in the platform user cache directory and is
reused without retuning on later starts. Use `--opencl-tuning=retune` after a
hardware change, or `--opencl-tuning=off` for the conservative portable
fallback. `--opencl-kernel`, `--opencl-nonces-per-item`, and
`--opencl-local-size` provide diagnostic overrides.
On macOS the cache is `~/Library/Caches/blake2b-miner/opencl-tuning.json`; on
Linux it is under `$XDG_CACHE_HOME` or `~/.cache`.

## Configuration

Edit `config.yaml`:

```yaml
stratum_url: "stratum+tcp://pool.acme.com:5575"
socks5_proxy: null # e.g. "127.0.0.1:25344"; proxy resolves the pool hostname
username: "wallet.worker"
password: "x"
threads: 0 # automatic; Datum both-mode reserves one logical CPU for Metal
device: both # cpu, gpu, or both
gpu_backend: auto # auto, metal, cuda, or opencl
gpu_devices: all # one index, a YAML list such as [0, 2], or all
gpu_batch_size: 1048576 # use 16777216 for balanced M4 Datum throughput
metal_tuning: auto # auto, off, or retune; applies only to Metal

reconnect_delay_seconds: 5
stats_interval_seconds: 5 # set to 0 to print stats only on SIGINFO (Ctrl-T)
```

Credentials may be embedded in the URL:

```yaml
stratum_url: "stratum+tcp://wallet.worker:x@pool.acme.com:5575"
```

Plain `stratum+tcp` does not encrypt credentials or jobs.
When `socks5_proxy` is set, connection and destination DNS resolution go through
that proxy. Failure is fail-closed; the miner does not retry the pool directly.

## Run

The miner implements the experimental BIP-110 profile-0 dialects from the
community DATUM gateway forks. It accepts both the current DATUM job built from
`coinb1` and extranonces and the older direct-mid lab job with a fixed zero
`extranonce2`:

```sh
target/release/blake2b-miner \
  --device both \
  --stratum-url=stratum+tcp://127.0.0.1:23334 \
  --username=local.worker \
  --password=x
```

CLI values override YAML values. Either spelling of the URL flag works:

```sh
target/release/blake2b-miner \
  --startum-url=stratum+tcp://pool.acme.com:5575 \
  --username=wallet.worker \
  --password=x
```

`--device` overrides the YAML device. `threads` is ignored in GPU-only mode.

The periodic console line includes `best_share`, the actual difficulty of the
strongest qualifying share found since the process started. A new record prints
its job, nonce, and hash immediately. The miner decodes the network target
from each job's `nBits`; a hash meeting it produces a prominent
`BLOCK CANDIDATE FOUND` message before the share is submitted. The gateway and
node remain authoritative for whether that candidate is accepted and added to
the chain.

```sh
target/release/blake2b-miner --device cpu
target/release/blake2b-miner --device gpu
target/release/blake2b-miner --device both
```

Inventory GPUs without loading the Stratum configuration or connecting to a
gateway:

```sh
target/release/blake2b-miner --list-devices --gpu-backend opencl
```

On macOS, `auto` selects Metal. Elsewhere, it prefers CUDA when that feature and
an NVIDIA device are available, then tries OpenCL when enabled. Explicitly
unavailable backends fail before workers or a Stratum session are started.

Run a three-second local benchmark without connecting to a pool:

```sh
target/release/blake2b-miner --benchmark --device both
```

Set a longer duration when tuning OpenCL without involving another backend:

```sh
target/release/blake2b-miner \
  --benchmark --benchmark-seconds=30 \
  --device=gpu --gpu-backend=opencl
```

On the Apple M4 used for this tuning pass, the 30-second OpenCL-only result at
a 16,777,216-nonce batch improved from 453.5 MH/s to 460.1 MH/s. At the default
1,048,576-nonce batch, eliminating per-batch synchronization overhead improved
the result from 341.5 MH/s to 409.9 MH/s. These figures are device-specific;
other OpenCL devices select and cache their own launch parameters.

On a 32-core Apple M5 Max, Metal reached about 1.66 GH/s with a
67,108,864-nonce batch. The automatic Metal tuner found no repeatable kernel
improvement over the legacy array/split x4, 64-thread selection, and retained
it. GPU-only mode sustained more total throughput than combining Metal with
all available CPU workers. Detailed measurements are in
[`docs/metal-m5-max.md`](docs/metal-m5-max.md).

## Wire format

DATUM accepts `[job_id, previous_asic, coinb1_or_mid, coinb2, branches,
version, nbits, ntime8, clean]`, hashes the resulting profile-0 ASIC input, and
submits `[username, job_id, extranonce2, ntime8, nonce8]`. This mode supports
profile 0 with a null XOR mask; it is not a general BIP-110 implementation.

## StartOS regtest lab

Installable package sources for the pinned BIP110 Bitcoin node and DATUM
gateway are in [`startos/`](startos/README.md). The included GitHub Actions
workflow builds x86_64 and aarch64 `.s9pk` artifacts remotely, so the C++
services do not need to be compiled on this Mac. The node uses a private
peerless regtest chain, pre-mines heights 1–19, and leaves the BLAKE2b height-20
activation block for this miner.

## Verify

```sh
./bin/test
cargo clippy --all-targets --all-features -- -D warnings
```
