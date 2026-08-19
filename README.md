# blake2b-apple-miner

Blake2b-256 Stratum miner for Apple silicon. The CPU hot loop hashes four
independent nonces at once using AArch64 NEON. The GPU backend runs a Metal
compute kernel with one nonce per thread. CPU and GPU workers reserve disjoint
nonce ranges from the same job.

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
username: "wallet.worker"
password: "x"
threads: 0 # all logical CPUs
device: both # cpu, gpu, or both
gpu_batch_size: 1048576

# Used by --normal. --sia and --datum use fixed 80-byte layouts.
nonce_offset: 32
nonce_size: 8
nonce_endian: little
hash_byte_order: little

reconnect_delay_seconds: 5
stats_interval_seconds: 5
```

Credentials may be embedded in the URL:

```yaml
stratum_url: "stratum+tcp://wallet.worker:x@pool.acme.com:5575"
```

Plain `stratum+tcp` does not encrypt credentials or jobs.

## Run

Sia mode implements the SiaMining Stratum V1 layout. It builds the arbitrary
transaction and right-sided Merkle path, then hashes Sia's 80-byte header with
the little-endian 64-bit nonce at byte 32.

```sh
target/release/blake2b-apple-miner --sia
```

Normal mode hashes a raw Blake2b-256 blob with the nonce layout from YAML:

```sh
target/release/blake2b-apple-miner --normal
```

DATUM mode implements the experimental BIP-110 profile-0 dialect from
`Maveth/datum_gateway`'s `bip110-pow-v2` branch. It hashes the gateway's direct
80-byte ASIC input and submits the fixed zero `extranonce2` required by that
lab protocol:

```sh
target/release/blake2b-apple-miner \
  --datum \
  --device both \
  --stratum-url=stratum+tcp://127.0.0.1:23334 \
  --username=local.worker \
  --password=x
```

Either spelling of the URL flag works. CLI values override YAML values:

```sh
target/release/blake2b-apple-miner \
  --sia \
  --startum-url=stratum+tcp://pool.acme.com:5575 \
  --username=wallet.worker \
  --password=x

target/release/blake2b-apple-miner \
  --normal \
  --stratum-url=stratum+tcp://pool.acme.com:5575
```

`--sia`, `--datum`, and `--normal` are mutually exclusive. One is required.
`--device` overrides the YAML device. `threads` is ignored in GPU-only mode.

```sh
target/release/blake2b-apple-miner --sia --device cpu
target/release/blake2b-apple-miner --sia --device gpu
target/release/blake2b-apple-miner --sia --device both
```

Run a three-second local benchmark without connecting to a pool:

```sh
target/release/blake2b-apple-miner --sia --benchmark --device both
```

## Normal-mode wire format

Normal mode intentionally rejects Bitcoin-style `mining.notify` jobs. A pool
must send a raw Blake2b job in one of these forms:

```json
{
  "id": null,
  "method": "mining.notify",
  "params": {
    "job_id": "job-42",
    "blob": "<hex, at most 128 bytes>",
    "target": "<big-endian target hex>",
    "nonce_offset": 32,
    "nonce_size": 8,
    "nonce_endian": "little",
    "hash_byte_order": "little"
  }
}
```

The compact array form is `[job_id, blob, target, clean_jobs]`. A preceding
`mining.set_target` may replace the per-job target. Shares use
`[username, job_id, nonce_hex]`.

Sia mode accepts the nine-parameter Sia Stratum notification and submits
`[username, job_id, extranonce2, ntime, nonce]`.

DATUM mode accepts `[job_id, previous_asic, mid, "", [], version, nbits,
ntime8, clean]`, hashes `previous_asic || nonce8_le || ntime8 || mid`, and
submits `[username, job_id, "0000000000000000", ntime8, nonce8]`. This mode
supports the gateway's profile 0 with a null XOR mask; it is not a general
BIP-110 implementation.

## StartOS regtest lab

Installable package sources for the pinned BIP110 Bitcoin node and DATUM
gateway are in [`startos/`](startos/README.md). The included GitHub Actions
workflow builds x86_64 and aarch64 `.s9pk` artifacts remotely, so the C++
services do not need to be compiled on this Mac. The node uses a private
peerless regtest chain, pre-mines heights 1–19, and leaves the BLAKE2b height-20
activation block for this miner's `--datum` mode.

## Verify

```sh
./bin/test
cargo clippy --all-targets --all-features -- -D warnings
```
