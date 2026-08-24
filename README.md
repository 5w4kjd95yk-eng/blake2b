# blake2b-apple-miner

DATUM BIP-110 profile-0 Blake2b-256 Stratum miner. On Apple silicon, the CPU
hot loop hashes four independent nonces at once using AArch64 NEON and the
default GPU backend runs a Metal compute kernel with four nonces per thread.
CPU and GPU workers reserve disjoint nonce ranges from the same job.

The GPU worker uses a backend-neutral synchronous interface for device
identity, job preparation, batch dispatch, and candidate collection. Metal is
the only implemented backend for now; CUDA support is being developed as an
opt-in backend and is not required by the default macOS build. Sprint 1
baseline and acceptance results are recorded in
[`docs/cuda-sprint-1.md`](docs/cuda-sprint-1.md).

Rust fits this job. It exposes AArch64 intrinsics without requiring assembly,
keeps the networking/configuration code memory-safe, and has no runtime or GC
in the hash loop.

## Build

```sh
cargo build --release
```

## Configuration

Edit `config.yaml`:

```yaml
stratum_url: "stratum+tcp://pool.acme.com:5575"
socks5_proxy: null # e.g. "127.0.0.1:25344"; proxy resolves the pool hostname
username: "wallet.worker"
password: "x"
threads: 0 # automatic; Datum both-mode reserves one logical CPU for Metal
device: both # cpu, gpu, or both
gpu_batch_size: 1048576 # use 16777216 for balanced M4 Datum throughput

reconnect_delay_seconds: 5
stats_interval_seconds: 5
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
target/release/blake2b-apple-miner \
  --device both \
  --stratum-url=stratum+tcp://127.0.0.1:23334 \
  --username=local.worker \
  --password=x
```

CLI values override YAML values. Either spelling of the URL flag works:

```sh
target/release/blake2b-apple-miner \
  --startum-url=stratum+tcp://pool.acme.com:5575 \
  --username=wallet.worker \
  --password=x
```

`--device` overrides the YAML device. `threads` is ignored in GPU-only mode.

```sh
target/release/blake2b-apple-miner --device cpu
target/release/blake2b-apple-miner --device gpu
target/release/blake2b-apple-miner --device both
```

Run a three-second local benchmark without connecting to a pool:

```sh
target/release/blake2b-apple-miner --benchmark --device both
```

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
