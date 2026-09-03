# Apple M5 Max Metal tuning

Measured on 2026-09-03 while tuning the DATUM Metal backend.

## Environment

- Hardware: Apple M5 Max, 32-core GPU, 18 logical CPUs, 36 GB unified memory
- Metal: Metal 4, runtime thread execution width 32
- Rust: Homebrew rustc/cargo 1.98.0
- Branch: `metal-m5-tuning`

## Existing-kernel baseline

Ten-second GPU-only screening runs with the original array/split x4 kernel
showed the batch-size knee clearly:

| Batch size | Throughput |
| ---: | ---: |
| 1,048,576 | 1,219.123 MH/s |
| 4,194,304 | 1,538.827 MH/s |
| 16,777,216 | 1,638.002 MH/s |
| 33,554,432 | 1,651.852 MH/s |
| 67,108,864 | 1,659.318 MH/s |
| 134,217,728 | 1,631.803 MH/s |
| 268,435,456 | 1,624.536 MH/s |

The 67,108,864-nonce batch was retained. Longer sequential runs showed a
material thermal decline, but reversing batch order did not reveal a reason to
use the larger, higher-latency batches.

## Kernel autotuning

The tuner validated and screened three implementations (`baseline`,
`scalar-split`, and `scalar-native`), one, two, and four nonces per thread, and
threadgroups from 32 through the pipeline limit. Its three finalists measured
1,637.461–1,638.880 MH/s during the larger confirmation pass, below the 1%
threshold required to displace the legacy selection.

Alternating 20-second production-batch trials likewise found no repeatable
gain. The legacy baseline averaged 1,643.521 MH/s across its two placements;
the nearest scalar finalist averaged 1,638.282 MH/s. Thermal state dominated
the remaining variation, so the cached M5 Max selection is `baseline`, four
nonces per thread, and a 64-thread group.

## Device mode

After sustained GPU loading, 20-second mode checks produced:

| Mode | Total | CPU | GPU |
| --- | ---: | ---: | ---: |
| GPU-only, first placement | 1,567.359 MH/s | 0 | 1,567.359 MH/s |
| Metal + 1 CPU worker | 1,527.001 MH/s | 20.755 MH/s | 1,506.245 MH/s |
| Metal + 17 CPU workers | 1,459.786 MH/s | 147.413 MH/s | 1,312.373 MH/s |
| GPU-only, final placement | 1,526.298 MH/s | 0 | 1,526.298 MH/s |

GPU-only is the local default because CPU participation reduced Metal
throughput and did not improve sustained total hashrate.
