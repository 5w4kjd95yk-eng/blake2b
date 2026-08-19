import { sdk } from './sdk'
import { rpcHostId, rpcPort } from './utils'

export const setInterfaces = sdk.setupInterfaces(async ({ effects }) => {
  await sdk.MultiHost.of(effects, rpcHostId).bindPort(rpcPort, {
    protocol: 'http',
    preferredExternalPort: rpcPort,
  })

  // Intentionally bridge-only: the DATUM package can reach RPC, but it is not
  // advertised as a LAN-facing user interface.
  return []
})
