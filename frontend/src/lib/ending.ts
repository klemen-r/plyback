import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { Chess } from 'chess.js'
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

type Mark = 'win' | 'mate' | 'resign' | 'time' | 'loss' | 'draw'

/** The outcome shown on the board: from the final position first, then the stored result. */
export function outcomeOf(g: { white: Side; black: Side; result: string }, finalFen: string):
  { white: Mark; black: Mark } | null {
  let position: Chess | null = null
  try { position = new Chess(finalFen) } catch { /* a variant position chess.js can't load */ }
  if (position?.isCheckmate()) return position.turn() === 'w' ? { white: 'mate', black: 'win' } : { white: 'win', black: 'mate' }
  if (position?.isStalemate()) return { white: 'draw', black: 'draw' }
  const w = g.white.result ?? '', b = g.black.result ?? ''
  if (DRAWS[w] || DRAWS[b] || g.result === '1/2-1/2') return { white: 'draw', black: 'draw' }
  const lost = (code: string): Mark => code === 'checkmated' ? 'mate' : code === 'resigned' ? 'resign' : code === 'timeout' ? 'time' : 'loss'
  if (w === 'win' || (!b && g.result === '1-0')) return { white: 'win', black: lost(b) }
  if (b === 'win' || (!w && g.result === '0-1')) return { white: lost(w), black: 'win' }
  return null
}

// Icon paths from Lucide (ISC), drawn on a 24-unit grid.
const ICON: Partial<Record<Mark, string>> = {
  win: '<path d="M11.562 3.266a.5.5 0 0 1 .876 0L15.39 8.87a1 1 0 0 0 1.516.294L21.183 5.5a.5.5 0 0 1 .798.519l-2.834 10.246a1 1 0 0 1-.956.734H5.81a1 1 0 0 1-.957-.734L2.02 6.02a.5.5 0 0 1 .798-.519l4.276 3.664a1 1 0 0 0 1.516-.294z"/><path d="M5 21h14"/>',
  resign: '<path d="M4 22V4a1 1 0 0 1 .4-.8A6 6 0 0 1 8 2c3 0 5 2 7.333 2q2 0 3.067-.8A1 1 0 0 1 20 4v10a1 1 0 0 1-.4.8A6 6 0 0 1 16 16c-3 0-5-2-8-2a6 6 0 0 0-4 1.528"/>',
  time: '<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2 2"/><path d="M5 3 2 6"/><path d="m22 6-3-3"/>',
}
const TEXT: Partial<Record<Mark, string>> = { mate: '#', draw: '½', loss: '0' }
const COLOR: Record<Mark, string> = { win: '#81b64c', mate: '#fa412d', resign: '#fa412d', time: '#fa412d', loss: '#fa412d', draw: '#8f969e' }

/** A badge in the square's top-right corner, like the move classification badges. */
function markSvg(mark: Mark): string {
  const inner = ICON[mark]
    ? `<g transform="translate(9 9) scale(0.92)" fill="none" stroke="#fff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round">${ICON[mark]}</g>`
    : `<text x="20" y="21" text-anchor="middle" dominant-baseline="central" font-family="Geist Variable,Arial,sans-serif" font-weight="800" font-size="28" fill="#fff">${TEXT[mark]}</text>`
  return `<g transform="translate(68 -8)"><circle cx="20" cy="20" r="19" fill="${COLOR[mark]}" stroke="#fff" stroke-width="3"/>${inner}</g>`
}

/** Badges on both kings in the final position: the winner's crown and how the other side lost, or ½ for a draw. */
export function endShapes(g: { white: Side; black: Side; result: string }, finalFen: string): DrawShape[] {
  const outcome = outcomeOf(g, finalFen)
  if (!outcome) return []
  const kings: Partial<Record<'K' | 'k', Key>> = {}
  finalFen.split(' ')[0].split('/').forEach((row, r) => {
    let file = 0
    for (const ch of row) {
      if (/\d/.test(ch)) { file += Number(ch); continue }
      if (ch === 'K' || ch === 'k') kings[ch] = `${'abcdefgh'[file]}${8 - r}` as Key
      file++
    }
  })
  const shapes: DrawShape[] = []
  if (kings.K) shapes.push({ orig: kings.K, customSvg: { html: markSvg(outcome.white) } })
  if (kings.k) shapes.push({ orig: kings.k, customSvg: { html: markSvg(outcome.black) } })
  return shapes
}
