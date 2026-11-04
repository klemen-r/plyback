import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { useAccount } from './account-context'
import { api } from './api'
import type { GameSummary, MoveRow } from './types'

/** The player's costliest classified move in a game (all moves when the player's side is unknown). */
export function useCostliestMove(g: GameSummary) {
  const { account } = useAccount()
  const detail = useQuery({ queryKey: ['game', g.id, account?.id, g.analyzed], queryFn: () => api.game(g.id, account?.id) })
  const moves = detail.data?.moves
  const worst = useMemo<MoveRow | undefined>(() => {
    const mine = g.user_color ? (moves ?? []).filter(m => m.color === g.user_color![0]) : (moves ?? [])
    return mine.filter(m => m.ep_loss !== undefined && m.classification).sort((a, b) => (b.ep_loss ?? 0) - (a.ep_loss ?? 0))[0]
  }, [moves, g.user_color])
  return { detail, worst }
}
