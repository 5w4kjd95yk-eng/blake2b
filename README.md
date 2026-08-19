# blake2b-apple-miner

CPU Blake2b-256 Stratum miner for Apple silicon. The hot loop hashes four
independent nonces at once using two AArch64 NEON `u64x2` vectors. The release
profile enables native Apple CPU tuning, fat LTO, and one codegen unit.

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

# Used by --normal. --sia fixes these to Sia's consensus layout.
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

`--sia` and `--normal` are mutually exclusive. One is required.

Run a three-second local benchmark without connecting to a pool:

```sh
target/release/blake2b-apple-miner --sia --benchmark
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

## Verify

```sh
./bin/test
cargo clippy --all-targets --all-features -- -D warnings
```
