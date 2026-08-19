import { IMPOSSIBLE, VersionInfo } from '@start9labs/start-sdk'

export const current = VersionInfo.of({
  version: '0.1.0:1',
  releaseNotes: {
    en_US: 'Initial experimental DATUM BIP110 regtest gateway package.',
  },
  migrations: {
    up: async () => {},
    down: IMPOSSIBLE,
  },
})
