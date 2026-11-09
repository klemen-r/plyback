import type { Key } from '@lichess-org/chessground/types'
import { Chess } from 'chess.js'

export interface VarStep { fen: string; san: string; from: Key; to: Key }
export interface Variation { baseFen: string; basePly: number; steps: VarStep[]; idx: number; source: 'user' | 'engine' }

export function buildSteps(fen: string, sans: string[]): VarStep[] {
  const chess = new Chess(fen)
  const out: VarStep[] = []
  for (const san of sans) {
    try {
      const m = chess.move(san)
      out.push({ fen: chess.fen(), san: m.san, from: m.from as Key, to: m.to as Key })
    } catch { break }
  }
  return out
}

/**
 * Plays an engine line from `fen` up to its `idx`th move. Inside a variation the line continues
 * from the step shown, so the moves that led there are kept; otherwise it starts a new variation
 * from the game position `basePly`.
 */
export function playEngineLine(variation: Variation | null, fen: string, basePly: number, sans: string[], idx: number): Variation | null {
  const steps = buildSteps(fen, sans)
  if (!steps.length) return null
  const at = Math.min(idx, steps.length - 1)
  if (!variation) return { baseFen: fen, basePly, steps, idx: at, source: 'engine' }
  const kept = variation.steps.slice(0, variation.idx + 1)
  return { ...variation, steps: [...kept, ...steps], idx: kept.length + at }
}
