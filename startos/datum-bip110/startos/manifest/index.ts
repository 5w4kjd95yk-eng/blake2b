import { setupManifest } from '@start9labs/start-sdk'
import { bitcoinDependencyDescription, long, short } from './i18n'

export const manifest = setupManifest({
  id: 'datum-bip110',
  title: 'DATUM BIP110 Gateway',
  license: 'MIT',
  packageRepo: 'https://github.com/Maveth/datum_gateway/tree/bip110-pow-v2',
  upstreamRepo: 'https://github.com/Maveth/datum_gateway/tree/bip110-pow-v2',
  marketingUrl:
    'https://github.com/Maveth/datum_gateway/blob/bip110-pow-v2/README.BIP110.md',
  donationUrl: null,
  description: { short, long },
  volumes: ['main'],
  images: {
    datum: {
      source: {
        dockerBuild: {
          buildArgs: {
            DATUM_COMMIT: '8d41b1338702bc42db23b84759a0efa73aa1487a',
            DATUM_TARBALL_SHA256:
              '1f1c015a4db608d47742821a212e5eca0102925105c81ac8815b8d01852118d1',
          },
        },
      },
      arch: ['x86_64', 'aarch64'],
    },
  },
  dependencies: {
    'bitcoin-bip110': {
      description: bitcoinDependencyDescription,
      optional: false,
      metadata: {
        title: 'Bitcoin BIP110 Lab',
        icon: 'https://raw.githubusercontent.com/Start9Labs/bitcoin-core-startos/51db7e317f48151a75b270dff49039b397048c80/dep-icon.svg',
      },
    },
  },
})
