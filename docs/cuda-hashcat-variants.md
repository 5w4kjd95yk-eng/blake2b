# CUDA Hashcat-inspired kernel variants

This branch adds opt-in DATUM CUDA experiments while retaining
`reference`, one nonce per thread, and 256 threads per block as the defaults.
No throughput measurements were taken while developing the variants.

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

## Hardware acceptance

After the shared machine is cleared for performance work, compare every
kernel using one nonce per thread first, then test widths 2 and 4 only for
correct variants without harmful resource use. Sweep block sizes 32, 64, 128,
256, 512, and 1024 at both the default and large batch sizes. Alternate the
reference and candidate runs, retain all samples, and keep the current default
until a winner is repeatable in both the local benchmark and a live DATUM job.
