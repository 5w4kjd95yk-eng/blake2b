# CUDA Hashcat-inspired kernel variants

This branch added opt-in DATUM CUDA experiments. Hardware measurements now
select `scalar`, one nonce per thread, and 512 threads per block as the defaults.

## Variants

- `reference` preserves the original message/state arrays, sigma lookup, and
  round loop.
- `scalar` names the ten live message words and sixteen state words and emits
  the twelve message schedules explicitly.
- `scalar-permute` additionally follows Hashcat's NVIDIA lowering for BLAKE2b
  rotations: `__byte_perm` for 16 and 24, a half swap for 32, and the normal
  constant rotate for 63.
- `scalar-permute-precompute` prepares the nonce-independent G functions 0,
  1, and 3 from round zero once per job. The device starts with G function 2,
  whose first message word is the nonce, and then completes the diagonal step.

Each variant is instantiated for 1, 2, and 4 sequential nonces per thread.
CUDA block size is independently selectable in warp-sized increments.

## Static compiler check

CUDA 13.1 successfully compiled all twelve kernel instantiations for `sm_89`.
The tuned kernels used 48 registers at widths 1 and 2. Width 4 used 56
registers for `scalar` and `scalar-permute`, and 64 for the precomputed variant.
None of the tuned kernels reported stack frames or spills. The reference
variant reported a 128-byte stack frame; its register counts were 54, 56, and
168 for widths 1, 2, and 4 respectively.

These figures describe compiler resource allocation, not mining performance.
They should be collected again for the incoming GPU's actual compute
capability before interpreting occupancy.

## Blackwell measurements

Measured on 2026-08-27 with an RTX 5070 Ti Laptop GPU (compute capability 12.0),
CUDA 13.1, and driver 591.84. All variants passed the CUDA-to-Rust boundary
comparison before benchmarking. A broad two-second sweep covered all kernels
at width 1 and block sizes 32 through 1024, followed by widths 2 and 4 at block
sizes 128, 256, and 512.

Longer five-second runs alternated the current reference kernel, the strongest
plain scalar configuration, and the strongest short-sweep result. Medians from
three samples were:

| Batch size | Reference | Scalar x1/block 512 | Permute-precompute x2/block 512 |
| ---: | ---: | ---: | ---: |
| 1,048,576 | 1,976.918 MH/s | 2,706.174 MH/s | 2,675.821 MH/s |
| 16,777,216 | 2,285.942 MH/s | 3,474.200 MH/s | 3,353.103 MH/s |

Scalar x1/block 512 improved median throughput by 36.9% at the default batch
and 52.0% at the large batch. Width 4 generally regressed. The explicit
permutation lowering did not improve plain scalar performance on this target,
and the precomputed width-2 candidate was less repeatable, so scalar x1/block
512 is the default.
