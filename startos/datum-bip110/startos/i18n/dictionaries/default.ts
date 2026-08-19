export const DEFAULT_LANG = 'en_US'

const dict = {
  'BIP110 Stratum': 0,
  'Stratum endpoint for the BIP110 DATUM miner': 1,
  'The BIP110 Stratum endpoint is ready': 2,
  'The BIP110 Stratum endpoint is not ready': 3,
} as const

export type I18nKey = keyof typeof dict
export type LangDict = Record<(typeof dict)[I18nKey], string>
export default dict
