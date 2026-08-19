# Bitcoin BIP110 Lab for StartOS

Experimental StartOS package for the `pow_hf_blake2b` Bitcoin branch, pinned to
commit `dedbfa8dd33e633426120f3608f489bc185aa6ba` and its verified source
tarball hash.

The service is deliberately regtest-only and peerless. Its first-start helper
mines real, validated SHA256d regtest blocks through height 19. BLAKE2b
activates at height 20, which is left for the DATUM gateway and external miner.
This avoids IBD without falsifying validation state.

Supported StartOS architectures: x86_64 and aarch64.
