import { IMPOSSIBLE, VersionInfo } from '@start9labs/start-sdk'

export const current = VersionInfo.of({
  version: '0.1.0:3',
  releaseNotes: {
    en_US:
      'Fix the BIP110 health check under the C.UTF-8 locale, and keep the corrected network-specific regtest RPC configuration.',
  },
  migrations: {
    up: async () => {},
    down: IMPOSSIBLE,
  },
})
