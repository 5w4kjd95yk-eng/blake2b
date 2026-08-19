import { setupManifest } from '@start9labs/start-sdk'
import { long, short } from './i18n'

export const manifest = setupManifest({
  id: 'bitcoin-bip110',
  title: 'Bitcoin BIP110 Lab',
  license: 'MIT',
  packageRepo: 'https://github.com/luke-jr/bitcoin/tree/pow_hf_blake2b',
  upstreamRepo: 'https://github.com/luke-jr/bitcoin/tree/pow_hf_blake2b',
  marketingUrl: 'https://github.com/bitcoinknots/bitcoin/pull/359',
  donationUrl: null,
  description: { short, long },
  volumes: ['main'],
  images: {
    bitcoind: {
      source: {
        dockerBuild: {
          buildArgs: {
            BITCOIN_COMMIT: 'dedbfa8dd33e633426120f3608f489bc185aa6ba',
            BITCOIN_TARBALL_SHA256:
              'f65b9b0cd2b3b57a8ccd18633472bc7dd63031ac1011df8f5263d39434e638fb',
          },
        },
      },
      arch: ['x86_64', 'aarch64'],
    },
  },
  dependencies: {},
})
