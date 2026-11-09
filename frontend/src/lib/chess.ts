import { Chess } from 'chess.js'
import type { Key } from '@lichess-org/chessground/types'

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


/** The live engine answers a finished game with lines that have no moves and no depth. */
export function isGameOverAnswer(lines: { san: string[]; depth: number }[]): boolean {
  return lines.length > 0 && lines.every(l => l.san.length === 0 && !l.depth)
}

/** How a finished position ended, for the live engine panel. */
export function gameOverText(fen: string): string {
  try {
    const chess = new Chess(fen)
    if (chess.isCheckmate()) return `Checkmate. ${chess.turn() === 'w' ? 'Black' : 'White'} wins.`
    if (chess.isStalemate()) return 'Stalemate. The game is drawn.'
    if (chess.isInsufficientMaterial()) return 'Drawn by insufficient material.'
    if (Number(fen.split(' ')[4]) >= 150) return 'Drawn by the 75-move rule.'
  } catch { /* not a position chess.js can read */ }
  return 'Game over.'
}
