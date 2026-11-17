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

export type VariationAction =
  | { type: 'start'; fen: string; basePly: number; sans: string[]; source: Variation['source'] }
  | { type: 'append'; san: string }
  | { type: 'continueWithLine'; from: Variation | null; fen: string; basePly: number; sans: string[]; idx: number }
  | { type: 'seek'; idx: number }
  | { type: 'exit' }

/** Keep the variation's origin and discard only moves after the cursor when branching. */
export function variationReducer(state: Variation | null, action: VariationAction): Variation | null {
  switch (action.type) {
    case 'start': {
      const steps = buildSteps(action.fen, action.sans)
      return steps.length ? { baseFen: action.fen, basePly: action.basePly, steps, idx: steps.length - 1, source: action.source } : state
    }
    case 'append': {
      if (!state) return state
      const fen = state.idx >= 0 ? state.steps[state.idx].fen : state.baseFen
      const added = buildSteps(fen, [action.san])
      if (!added.length) return state
      const steps = [...state.steps.slice(0, state.idx + 1), ...added]
      return { ...state, steps, idx: steps.length - 1, source: 'user' }
    }
    case 'continueWithLine':
      return playEngineLine(action.from, action.fen, action.basePly, action.sans, action.idx) ?? state
    case 'seek':
      return state && action.idx >= -1 && action.idx < state.steps.length ? { ...state, idx: action.idx } : state
    case 'exit':
      return null
  }
}
