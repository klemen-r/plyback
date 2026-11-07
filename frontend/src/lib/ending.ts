import type { GameSummary } from './types'

/** How a game ended, from the per-side result codes that Chess.com and PGN imports store. */
const HOW: Record<string, string> = {
  checkmated: 'checkmate', resigned: 'resignation', timeout: 'time', abandoned: 'abandonment',
  kingofthehill: 'king of the hill', threecheck: 'three checks', bughousepartnerlose: 'partner loss',
}
const DRAWS: Record<string, string> = {
  agreed: 'agreement', repetition: 'repetition', stalemate: 'stalemate', insufficient: 'insufficient material',
  '50move': 'the 50-move rule', timevsinsufficient: 'time against insufficient material',
}

export interface Ending {
  /** One or two words for tables: "Checkmate", "Time", "Repetition". */
  short: string
  /** A sentence fragment from the account's side when known: "Won by checkmate", "Lost on time". */
  text: string
}

function by(how: string) {
  return how === 'time' ? 'on time' : how === 'abandonment' ? 'by abandonment' : `by ${how}`
}

type Side = { result: string | null }
export function endingOf(g: { white: Side; black: Side } & Pick<GameSummary, 'user_color' | 'termination'>): Ending | null {
  const w = g.white.result ?? '', b = g.black.result ?? ''
  const draw = DRAWS[w] ?? DRAWS[b]
  if (draw) {
    const short = draw === 'agreement' ? 'Agreement' : draw === 'the 50-move rule' ? '50-move rule'
      : draw === 'time against insufficient material' ? 'Time vs material' : draw[0].toUpperCase() + draw.slice(1)
    return { short, text: draw === 'stalemate' ? 'Draw by stalemate' : `Draw by ${draw}` }
  }
  const whiteWon = w === 'win', blackWon = b === 'win'
  if (!whiteWon && !blackWon) {
    // No result codes (for example an imported PGN without them): fall back to the stored text.
    return g.termination ? { short: g.termination, text: g.termination } : null
  }
  const how = HOW[whiteWon ? b : w]
  if (!how) {
    // A win without a known reason (Chess.com's generic "lose", or a PGN without a Termination).
    const short = whiteWon ? 'White won' : 'Black won'
    if (g.termination) return { short, text: g.termination }
    return { short, text: g.user_color ? ((g.user_color === 'white') === whiteWon ? 'Won' : 'Lost') : short }
  }
  const short = how === 'time' ? 'Time' : how[0].toUpperCase() + how.slice(1)
  if (!g.user_color) return { short, text: `${whiteWon ? 'White' : 'Black'} won ${by(how)}` }
  const won = (g.user_color === 'white') === whiteWon
  if (how === 'resignation') return { short, text: won ? 'Won by resignation' : 'You resigned' }
  if (how === 'abandonment') return { short, text: won ? 'Won, opponent left' : 'You left the game' }
  return { short, text: `${won ? 'Won' : 'Lost'} ${by(how)}` }
}
