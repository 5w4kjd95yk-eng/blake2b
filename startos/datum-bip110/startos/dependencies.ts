import { sdk } from './sdk'

export const setDependencies = sdk.setupDependencies(async () => ({
  'bitcoin-bip110': {
    kind: 'running',
    versionRange: '>=0.1.0:1 <0.2.0',
    healthChecks: ['bitcoind'],
  },
}))
