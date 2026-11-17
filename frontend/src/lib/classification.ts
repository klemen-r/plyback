import type { Classification } from './types'

export interface ClassMeta {
  label: string
  color: string
  symbol: string
  description: string
}

export const CLASS_META: Record<Classification, ClassMeta> = {
  brilliant: { label: 'Brilliant', color: '#1bada6', symbol: '!!', description: 'A sound sacrifice that is also the best move.' },
  great: { label: 'Great', color: '#5c8bb0', symbol: '!', description: 'The only good move: every alternative is clearly worse.' },
  best: { label: 'Best', color: '#81b64c', symbol: '★', description: "The engine's top choice." },
  excellent: { label: 'Excellent', color: '#98bc4b', symbol: '✓', description: 'Loses 2% winning chances or less.' },
  good: { label: 'Good', color: '#97af8b', symbol: '✓', description: 'Loses 2–5% winning chances.' },
  book: { label: 'Book', color: '#a88865', symbol: '≡', description: 'Known opening theory.' },
  forced: { label: 'Forced', color: '#8f969e', symbol: '□', description: 'The only legal move.' },
  inaccuracy: { label: 'Inaccuracy', color: '#f7c045', symbol: '?!', description: 'Loses 5–10% winning chances.' },
  mistake: { label: 'Mistake', color: '#ffa459', symbol: '?', description: 'Loses 10–20% winning chances.' },
  miss: { label: 'Miss', color: '#ff7769', symbol: '✗', description: "Failed to punish your opponent's error." },
  blunder: { label: 'Blunder', color: '#fa412d', symbol: '??', description: 'Loses more than 20% winning chances.' },
}

export const CLASS_ORDER: Classification[] = [
  'brilliant', 'great', 'best', 'excellent', 'good', 'book', 'inaccuracy', 'mistake', 'miss', 'blunder',
]

/** Error labels, most severe first. */
export const ERROR_CLASSES = ['blunder', 'mistake', 'miss', 'inaccuracy'] as const satisfies readonly Classification[]

/** SVG badge placed in the corner of a board square (chessground customSvg, 100×100 viewBox). */
export function classBadgeSvg(c: Classification): string {
  const m = CLASS_META[c]
  const fs = m.symbol.length > 1 ? 26 : 32
  return `<g transform="translate(68 -8)"><circle cx="20" cy="20" r="19" fill="${m.color}" stroke="#fff" stroke-width="3"/>` +
    `<text x="20" y="21" text-anchor="middle" dominant-baseline="central" font-family="Geist Variable,Arial,sans-serif" font-weight="800" font-size="${fs}" fill="#fff">${m.symbol}</text></g>`
}

export const PHASE_LABEL: Record<string, string> = { opening: 'Opening', middlegame: 'Middlegame', endgame: 'Endgame' }

export const REASON_LABEL: Record<string, string> = {
  checkmate: 'Checkmate', resignation: 'Resignation', time: 'Time', abandonment: 'Abandonment', other: 'Other',
  agreed: 'Agreement', repetition: 'Repetition', stalemate: 'Stalemate', insufficient: 'Insufficient material',
  '50move': '50-move rule', timevsinsufficient: 'Timeout vs. insufficient material',
}
