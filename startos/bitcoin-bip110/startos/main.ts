import { i18n } from './i18n'
import { sdk } from './sdk'

export const main = sdk.setupMain(async ({ effects }) => {
  const bitcoindSub = await sdk.SubContainer.eager(
    effects,
    { imageId: 'bitcoind' },
    sdk.Mounts.of().mountVolume({
      volumeId: 'main',
      subpath: null,
      mountpoint: '/data',
      readonly: false,
    }),
    'bitcoind-sub',
  )

  return sdk.Daemons.of(effects).addDaemon('bitcoind', {
    subcontainer: bitcoindSub,
    exec: {
      command: ['/usr/local/bin/bitcoin-bip110-entrypoint'],
      sigtermTimeout: 120_000,
    },
    ready: {
      display: i18n('BIP110 Regtest'),
      fn: async () => {
        const result = await bitcoindSub.exec([
          'bitcoin-cli',
          '-datadir=/data',
          'getblockcount',
        ])
        const height = Number.parseInt(String(result.stdout).trim(), 10)

        if (result.exitCode !== 0 || !Number.isFinite(height)) {
          return {
            message: i18n('Bitcoin RPC is starting'),
            result: 'starting' as const,
          }
        }

        if (height < 19) {
          return {
            message: i18n('Bootstrapping regtest blocks'),
            result: 'loading' as const,
          }
        }

        return {
          message: i18n('Ready for BLAKE2b mining at height ${height}', {
            // StartOS may set LANG=C.UTF-8, which is not accepted by Intl.
            // Passing display-only values as text avoids locale formatting.
            height: String(height),
          }),
          result: 'success' as const,
        }
      },
    },
    requires: [],
  })
})
