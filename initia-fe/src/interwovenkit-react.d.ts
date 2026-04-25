import '@initia/interwovenkit-react'
import type { Chain } from '@initia/initia-registry-types'

declare module '@initia/interwovenkit-react' {
  interface Config {
    customChains?: Chain[]
  }
}
