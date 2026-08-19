# Bitcoin BIP110 Lab

This package is an experimental, regtest-only build of Luke Dashjr's
`pow_hf_blake2b` branch. It is not a Bitcoin mainnet node.

The package explicitly accepts the BIP110/RDTS rules at compile time. That
choice is limited to this clearly labelled test service and removes the
runtime consent prompt from its headless daemon.

On first start it creates and validates 19 ordinary regtest blocks locally.
There are no peers and therefore no Initial Block Download. Height 20 is left
unmined because it is the configured BLAKE2b activation height.

Install and start this service before DATUM BIP110 Gateway. The health check is
green when the node is at least height 19 and ready to provide the activation
template. The companion gateway discovers the RPC endpoint automatically.

The bootstrap payout address is disposable and has no wallet in this package.
All regtest coins are valueless.
