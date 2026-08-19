# DATUM BIP110 Gateway

This experimental service requires Bitcoin BIP110 Lab. StartOS wires its RPC
connection automatically and waits until the node has bootstrapped height 19.

Open the service's Interfaces page and copy the BIP110 Stratum address. On the
Mac that runs the miner, use:

```sh
target/release/blake2b-apple-miner \
  --datum \
  --device both \
  --startum-url=stratum+tcp://STARTOS_ADDRESS:23334 \
  --username=local.worker \
  --password=x
```

Replace the example address with the exact non-SSL address StartOS displays;
the external port may differ when port 23334 is already allocated.

The gateway uses minimum share difficulty 1, disables connection to an
external DATUM pool, and pays only a disposable regtest address. A submitted
height-20 block is a real block on this private regtest chain, but its coins
have no value.
