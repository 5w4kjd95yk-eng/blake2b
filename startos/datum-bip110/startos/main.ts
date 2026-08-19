import { i18n } from './i18n'
import { sdk } from './sdk'
import {
  bitcoinPackageId,
  bitcoinRpcHostId,
  bitcoinRpcPort,
  stratumPort,
} from './utils'

export const main = sdk.setupMain(async ({ effects }) => {
  const rpcAddress = await sdk.host
    .getBridgeAddress(effects, {
      packageId: bitcoinPackageId,
      hostId: bitcoinRpcHostId,
      internalPort: bitcoinRpcPort,
    })
    .const()

  if (!rpcAddress) {
    throw new Error('Bitcoin BIP110 RPC binding is unavailable')
  }

  const datumSub = await sdk.SubContainer.eager(
    effects,
    { imageId: 'datum' },
    sdk.Mounts.of().mountVolume({
      volumeId: 'main',
      subpath: null,
      mountpoint: '/data',
      readonly: false,
    }),
    'datum-sub',
  )

  return sdk.Daemons.of(effects).addDaemon('datum', {
    subcontainer: datumSub,
    exec: {
      command: ['/usr/local/bin/datum-bip110-entrypoint', rpcAddress],
      sigtermTimeout: 30_000,
    },
    ready: {
      display: i18n('BIP110 Stratum'),
      fn: () =>
        sdk.healthCheck.checkPortListening(effects, stratumPort, {
          successMessage: i18n('The BIP110 Stratum endpoint is ready'),
          errorMessage: i18n('The BIP110 Stratum endpoint is not ready'),
        }),
    },
    requires: [],
  })
})
