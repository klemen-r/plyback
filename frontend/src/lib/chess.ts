import { Chess } from 'chess.js'
import type { Key } from 'chessground/types'

export function legalDests(fen: string): Map<Key, Key[]> {
  const dests = new Map<Key, Key[]>()
  try {
    const chess = new Chess(fen)
    for (const m of chess.moves({ verbose: true })) {
      const arr = dests.get(m.from as Key) ?? []
      if (!arr.includes(m.to as Key)) arr.push(m.to as Key)
      dests.set(m.from as Key, arr)
    }
  } catch { /* invalid fen */ }
  return dests
}

export function sanToSquares(fen: string, san: string | null | undefined): [Key, Key] | null {
  if (!san) return null
  try {
    const m = new Chess(fen).move(san)
    return [m.from as Key, m.to as Key]
  } catch {
    return null
  }
}

