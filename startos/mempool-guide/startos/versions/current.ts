import { IMPOSSIBLE, VersionInfo } from '@start9labs/start-sdk'

export const current = VersionInfo.of({
  version: '3.4.0-dev.20260805:1',
  releaseNotes: {
    en_US:
      'Initial Mempool Guide package for StartOS. Built from Retropex/mempool commit a4c204dc513f05429638ffad9a84d627dde74c07 with BIP110 signaling and violation visualizations. Installs independently from the official Mempool package.',
  },
  migrations: {
    up: async () => {},
    down: IMPOSSIBLE,
  },
})
