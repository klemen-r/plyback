import { createContext, useContext } from 'react'
import type { Account } from './types'

interface Ctx {
  accounts: Account[]
  account: Account | null
  loading: boolean
  error: unknown
  select: (id: number) => void
}

export const AccountCtx = createContext<Ctx>({ accounts: [], account: null, loading: true, error: null, select: () => {} })
export const useAccount = () => useContext(AccountCtx)
