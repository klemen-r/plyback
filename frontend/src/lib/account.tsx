import { useQuery } from '@tanstack/react-query'
import { useMemo, useState, type ReactNode } from 'react'
import { api } from './api'
import { AccountCtx } from './account-context'

const KEY = 'ci.account'

function readStored(): number | null {
  try {
    const v = localStorage.getItem(KEY)
    return v ? Number(v) : null
  } catch {
    return null
  }
}

export function AccountProvider({ children }: { children: ReactNode }) {
  const [selected, setSelected] = useState<number | null>(readStored)
  const q = useQuery({
    queryKey: ['accounts'],
    queryFn: api.accounts,
    refetchInterval: query => (query.state.data?.some(a => a.sync_status === 'syncing' || a.queued > 0) ? 4000 : 15000),
  })
  const accounts = useMemo(() => q.data ?? [], [q.data])
  const account = accounts.find(a => a.id === selected) ?? accounts[0] ?? null

  const select = (id: number) => {
    setSelected(id)
    try { localStorage.setItem(KEY, String(id)) } catch { /* storage unavailable */ }
  }
  return (
    <AccountCtx.Provider value={{ accounts, account, loading: q.isLoading, error: q.error, select }}>
      {children}
    </AccountCtx.Provider>
  )
}


