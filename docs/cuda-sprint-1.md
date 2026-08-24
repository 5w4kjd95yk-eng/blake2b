# CUDA Sprint 1 baseline

Measured on 2026-08-23 before and after the GPU backend-boundary refactor.

## Environment

- Hardware: Apple M4, arm64
- macOS: 26.6.1
- Rust: rustc 1.97.1, cargo 1.97.1 (Homebrew)
- Local configuration: `threads: 0`, `gpu_batch_size: 16777216`
- CPU-only benchmark workers: 10
- Mixed CPU/Metal workers: 9 CPU workers and one Metal worker
- Benchmark duration: three seconds per sample
- Baseline commit: `734c6e5`
- Refactor commit: `82cbada`

Release binaries for both commits were built into separate target directories
and run in alternating order from the same working configuration.

## Release benchmark

| Backend | Baseline samples (MH/s) | Refactor samples (MH/s) | Baseline median | Refactor median | Median change |
| --- | --- | --- | ---: | ---: | ---: |
| CPU | 141.524, 141.026, 141.648, 142.821, 142.658, 141.911, 142.737, 141.764 | 138.003, 141.561, 140.775, 142.389, 142.896, 142.901, 143.108, 142.364 | 141.838 | 142.377 | +0.38% |
| Metal | 485.846, 486.835, 486.826 | 486.979, 487.072, 486.986 | 486.826 | 486.986 | +0.03% |

The CPU sample ranges overlap. The refactor's 138.003 MH/s sample was an
isolated low result; using all samples, its mean was 0.18% below baseline and
its median was 0.38% above baseline. Metal was effectively unchanged.

## Correctness and live acceptance

The existing `metal_matches_reference_for_datum_layout` comparison passed on
both commits. On the refactor commit, `./bin/test` passed all 19 tests, strict
Clippy passed, and the release build completed. The default release binary had
no CUDA or NVIDIA linkage.

A 30-second run against the local DATUM gateway using the working
`config.yaml`:

- Connected and processed three job epochs.
- Submitted 7 accepted shares and received 0 rejections.
- Reported 618.502 to 623.935 MH/s combined during the observation window.
- Stopped cleanly with Ctrl-C and exit status 0.

No shader, dispatch geometry, nonce reservation, or result-handling changes
were made in Sprint 1.
