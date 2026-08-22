# StartOS packages

This directory contains three StartOS SDK 2.0 packages:

- `bitcoin-bip110`: an isolated `pow_hf_blake2b` regtest node.
- `datum-bip110`: the matching `bip110-pow-v2` DATUM gateway.
- `mempool-guide`: the x86_64 mempool.guide/Retropex explorer, packaged under
  a separate service id so it can coexist with Start9's official Mempool.

The Bitcoin and DATUM packages are a pair and support x86_64 and aarch64. The
Mempool Guide package is independent and currently targets x86_64 only. All
three packages pin immutable source commits and verify source tarball hashes.

They use Start SDK 2.0.9 and target StartOS 0.4.0-beta.10. A server still on
the 0.3.5 generation must be upgraded before it can sideload these packages.

## No IBD

The Bitcoin package does not fake synchronization or bypass block validation.
It has no peers and runs a private regtest chain, so there is no mainnet IBD.
On first start it uses Bitcoin's own RPC to mine and validate heights 1 through
19. It stops there because `blake2b@20` is the activation setting. The DATUM
gateway supplies the height-20 BLAKE2b job to the external miner.

## Build remotely with GitHub Actions

The repository workflow `.github/workflows/startos-packages.yml` builds all
installable artifacts on GitHub-hosted runners, including Mempool Guide for
x86_64.

- Bitcoin BIP110 for x86_64
- Mempool Guide for x86_64

Push this repository to GitHub, open **Actions**, select **Build StartOS
Packages**, and choose **Run workflow**. Download the artifact for the package
you want. You do not need Docker, CMake, or a C++ toolchain on the Mac.

The workflow uses an ephemeral signing key because these are experimental
sideload packages. It does not publish them to a registry.

## Install and run

1. Sideload and start the matching `bitcoin-bip110` package.
2. Wait for its `BIP110 Regtest` health check to turn green at height 19.
3. Sideload and start the matching `datum-bip110` package. StartOS installs the
   declared Bitcoin dependency and wires RPC over its internal bridge.
4. In DATUM BIP110 Gateway's Interfaces page, copy the BIP110 Stratum address.
5. On the Apple Silicon Mac, run:

   ```sh
   target/release/blake2b-apple-miner \
     --datum \
     --device both \
     --startum-url='stratum+tcp://ADDRESS_SHOWN_BY_STARTOS' \
     --username=local.worker \
     --password=x
   ```

The intentionally misspelled `--startum-url` is supported as an alias; the
correct `--stratum-url` spelling works too.

## Fixed lab configuration

The packages regenerate these settings at every start:

- Network: regtest only; peer discovery and listening disabled.
- BLAKE2b activation: height 20.
- Required headline / primary coinbase tag: `BIP110-LAB`.
- Bitcoin RPC: port 18443, bridge-only on StartOS.
- Gateway Stratum: raw TCP port 23334 (StartOS may assign another external
  port if it is already occupied).
- Minimum share difficulty: 1.
- External DATUM pool: disabled.
- Payout: disposable testnet/regtest Base58 address with no packaged wallet.

These are deliberately not production or mainnet packages. Both upstream
branches are experimental and may be force-pushed; update the commit and
tarball hash together when testing a newer protocol revision.
