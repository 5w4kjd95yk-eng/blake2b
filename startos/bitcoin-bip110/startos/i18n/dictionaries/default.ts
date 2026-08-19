export const DEFAULT_LANG = 'en_US'

const dict = {
  'BIP110 Regtest': 0,
  'Bitcoin RPC is starting': 1,
  'Bootstrapping regtest blocks': 2,
  'Ready for BLAKE2b mining at height ${height}': 3,
} as const

export type I18nKey = keyof typeof dict
export type LangDict = Record<(typeof dict)[I18nKey], string>
export default dict
