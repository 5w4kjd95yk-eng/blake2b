import { setupManifest } from '@start9labs/start-sdk'
import {
  bitcoindDescription,
  clnDescription,
  electrsDescription,
  fulcrumDescription,
  lndDescription,
  long,
  short,
  torDescription,
} from './i18n'

export const manifest = setupManifest({
  id: 'mempool-guide',
  title: 'Mempool Guide',
  license: 'AGPL',
  packageRepo:
    'https://github.com/krzyczak/blake2b/tree/master/startos/mempool-guide',
  upstreamRepo: 'https://github.com/Retropex/mempool',
  marketingUrl: 'https://mempool.guide',
  donationUrl: null,
  description: { short, long },
  volumes: ['cache', 'db', 'config', 'startos'],
  images: {
    frontend: {
      source: {
        dockerBuild: {
          dockerfile: 'Dockerfile.frontend',
          workdir: '.',
          buildArgs: {
            MEMPOOL_GUIDE_COMMIT: 'a4c204dc513f05429638ffad9a84d627dde74c07',
            MEMPOOL_GUIDE_TARBALL_SHA256:
              'bbfa0df6e17d4a071fe8610b620c0bf254bd54b06f96c13725d44a5408fdbac0',
          },
        },
      },
      arch: ['x86_64'],
    },
    backend: {
      source: {
        dockerBuild: {
          dockerfile: 'Dockerfile.backend',
          workdir: '.',
          buildArgs: {
            MEMPOOL_GUIDE_COMMIT: 'a4c204dc513f05429638ffad9a84d627dde74c07',
            MEMPOOL_GUIDE_TARBALL_SHA256:
              'bbfa0df6e17d4a071fe8610b620c0bf254bd54b06f96c13725d44a5408fdbac0',
            GEOIP_COMMIT: '807ed1550b408eb13b680a6805df80a7a61f56ac',
            GEOIP_CITY_SHA256:
              '04d5e8e0e26f3ff50355a6dc02cdcc58b5f4bb09dbfbb3d582eb8fa941365176',
            GEOIP_ASN_SHA256:
              'c716338a3eef8dbac1382acf484c2d9e9f506bacba44a470d23354c81d538947',
          },
        },
      },
      arch: ['x86_64'],
    },
    mariadb: {
      source: {
        dockerTag: 'mariadb:10.4.34',
      },
      arch: ['x86_64'],
    },
  },
  dependencies: {
    bitcoind: {
      description: bitcoindDescription,
      optional: false,
      metadata: {
        title: 'Bitcoin',
        icon: 'https://raw.githubusercontent.com/Start9Labs/bitcoin-core-startos/refs/heads/30.x/dep-icon.svg',
      },
    },
    electrs: {
      description: electrsDescription,
      optional: true,
      metadata: {
        title: 'Electrs',
        icon: 'https://raw.githubusercontent.com/Start9Labs/electrs-startos/refs/heads/master/icon.svg',
      },
    },
    fulcrum: {
      description: fulcrumDescription,
      optional: true,
      metadata: {
        title: 'Fulcrum',
        icon: 'https://raw.githubusercontent.com/Start9Labs/fulcrum-startos/master/icon.png',
      },
    },
    'c-lightning': {
      description: clnDescription,
      optional: true,
      metadata: {
        title: 'Core Lightning',
        icon: 'https://raw.githubusercontent.com/Start9Labs/cln-startos/refs/heads/master/icon.svg',
      },
    },
    lnd: {
      description: lndDescription,
      optional: true,
      metadata: {
        title: 'LND',
        icon: 'https://raw.githubusercontent.com/Start9Labs/lnd-startos/refs/heads/master/icon.svg',
      },
    },
    tor: {
      description: torDescription,
      optional: true,
      metadata: {
        title: 'Tor',
        icon: 'https://raw.githubusercontent.com/Start9Labs/tor-startos/65faea17febc739d910e8c26ff4e61f6333487a8/icon.svg',
      },
    },
  },
})
