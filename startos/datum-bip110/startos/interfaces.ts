import { i18n } from './i18n'
import { sdk } from './sdk'
import { stratumHostId, stratumPort } from './utils'

export const setInterfaces = sdk.setupInterfaces(async ({ effects }) => {
  const stratumMulti = sdk.MultiHost.of(effects, stratumHostId)
  const stratumOrigin = await stratumMulti.bindPort(stratumPort, {
    protocol: null,
    preferredExternalPort: stratumPort,
    addSsl: null,
    secure: { ssl: false },
  })
  const stratum = sdk.createInterface(effects, {
    name: i18n('BIP110 Stratum'),
    id: 'stratum',
    description: i18n('Stratum endpoint for the BIP110 DATUM miner'),
    type: 'api',
    masked: false,
    schemeOverride: { ssl: 'stratum+tcp', noSsl: 'stratum+tcp' },
    username: null,
    path: '',
    query: {},
  })

  return [await stratumOrigin.export([stratum])]
})
