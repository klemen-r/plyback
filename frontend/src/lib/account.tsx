import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
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
  const qc = useQueryClient()
  // One shared status timer for the shell and pages. The revision also detects
  // a re-review that starts and finishes between polls without changing counts.
  const status = useQuery({
    queryKey: ['status'], queryFn: api.status,
    refetchInterval: query => {
      const s = query.state.data
      return s && !s.queue.running && !s.queue.queued && !s.sync.current_account ? 15000 : 5000
    },
  })
  const previousRevision = useRef<number | undefined>(undefined)
  useEffect(() => {
    const revision = status.data?.data_revision
    if (revision === undefined || revision === previousRevision.current) return
    const changed = previousRevision.current !== undefined
    previousRevision.current = revision
    if (!changed) return
    for (const key of ['accounts', 'games', 'game', 'insights', 'openings', 'repertoire']) {
      void qc.invalidateQueries({ queryKey: [key] })
    }
    // An active Practice session must keep its current puzzle and board.
    void qc.invalidateQueries({ queryKey: ['practice'], refetchType: 'none' })
  }, [qc, status.data?.data_revision])
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


