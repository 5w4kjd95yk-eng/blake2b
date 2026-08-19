import { IMPOSSIBLE, VersionInfo } from '@start9labs/start-sdk'

export const current = VersionInfo.of({
  version: '0.1.0:2',
  releaseNotes: {
    en_US:
      'Fix the generated regtest configuration so the network-specific RPC bind and port settings are accepted by bitcoind.',
  },
  migrations: {
    up: async () => {},
    down: IMPOSSIBLE,
  },
})
