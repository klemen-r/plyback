import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { api } from './api'

/** Debounce navigation while retaining cached lines and cancelling disabled analysis. */
export function useLiveEngine(fen: string, depth: number, enabled: boolean) {
  const qc = useQueryClient()
  const [settledFen, setSettledFen] = useState('')
  useEffect(() => {
    const timer = setTimeout(() => setSettledFen(fen), 150)
    return () => clearTimeout(timer)
  }, [fen])
  const live = useQuery({
    queryKey: ['engine', fen, depth],
    queryFn: ({ signal }) => api.engine(fen, depth, 3, signal),
    enabled: enabled && settledFen === fen,
    staleTime: Infinity,
  })
  useEffect(() => {
    if (!enabled) void qc.cancelQueries({ queryKey: ['engine', fen, depth], exact: true })
  }, [qc, fen, depth, enabled])
  return live
}
