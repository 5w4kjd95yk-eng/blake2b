# DATUM BIP110 Gateway for StartOS

Experimental StartOS wrapper for Maveth's `bip110-pow-v2` DATUM Gateway branch,
pinned to commit `8d41b1338702bc42db23b84759a0efa73aa1487a` and its verified
source tarball hash.

The service requires the companion `bitcoin-bip110` package. It discovers the
node's StartOS bridge address at runtime and exports the BIP110 Stratum dialect
on raw TCP. No remote DATUM pool is configured.

Supported StartOS architectures: x86_64 and aarch64.
