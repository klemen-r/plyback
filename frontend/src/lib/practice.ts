import type { Key } from '@lichess-org/chessground/types'
import { Chess } from 'chess.js'
import { DAY } from './format'
import type { InteractiveResult, PracticeItem, PracticeResult, PracticeTheme } from './types'

export const PRACTICE_THEMES: [PracticeTheme, string][] = [
  ['mate-in-1', 'Mate in 1'], ['mate-in-2', 'Mate in 2'], ['mate-in-3-plus', 'Mate in 3+'],
  ['back-rank-mate', 'Back-rank mate'], ['fork', 'Fork'], ['knight-fork', 'Knight fork'],
  ['pin', 'Pin'], ['skewer', 'Skewer'], ['discovered-attack', 'Discovered attack'],
  ['hanging-piece', 'Hanging piece'], ['trapped-piece', 'Trapped piece'], ['promotion', 'Promotion'], ['defensive', 'Defensive'],
]
export const PRACTICE_THEME_LABEL = Object.fromEntries(PRACTICE_THEMES) as Record<PracticeTheme, string>

export type PracticeRange = 'all' | 'today' | '7' | '30'
type PuzzleStatus = 'solving' | 'checking' | 'solved' | 'revealed'
interface PuzzleState {
  status: PuzzleStatus
  solution: PracticeResult | null
  played: string[]
  liveLine: string[] | null
  tries: number
  wrong: string | null
  hint: boolean
  note: string | null
  step: number | null
  trying: { fen: string; from: Key; to: Key } | null
  error: unknown
  typed: string
  typedError: string | null
  copied: 'yes' | 'failed' | null
  lineFor: boolean
}

type PuzzleAction =
  | { type: 'setting'; fullLine: boolean }
  | { type: 'answer'; result: PracticeResult }
  | { type: 'play'; moves: string[] }
  | { type: 'reject'; san: string }
  | { type: 'checking' }
  | { type: 'alternative'; line: string[]; note: string }
  | { type: 'finish'; status: 'solved' | 'revealed' }
  | { type: 'trying'; move: PuzzleState['trying'] }
  | { type: 'error'; error: unknown }
  | { type: 'hint' }
  | { type: 'seek'; step: number }
  | { type: 'type'; text: string }
  | { type: 'typedError'; text: string }
  | { type: 'copy'; result: 'yes' | 'failed' }

export function initialPuzzleState(fullLine: boolean): PuzzleState {
  return {
    status: 'solving', solution: null, played: [], liveLine: null, tries: 0, wrong: null,
    hint: false, note: null, step: null, trying: null, error: null, typed: '',
    typedError: null, copied: null, lineFor: fullLine,
  }
}

export function practiceReducer(state: PuzzleState, action: PuzzleAction): PuzzleState {
  switch (action.type) {
    case 'setting': return !state.played.length && !state.solution ? { ...state, lineFor: action.fullLine } : state
    case 'answer': return { ...state, solution: action.result }
    case 'play': return { ...state, played: action.moves, wrong: null, hint: false }
    case 'reject': return { ...state, status: 'solving', wrong: action.san, tries: state.tries + 1 }
    case 'checking': return { ...state, status: 'checking' }
    case 'alternative': return { ...state, liveLine: action.line, note: action.note, status: 'solving' }
    case 'finish': return { ...state, status: action.status, trying: null, error: null, ...(action.status === 'revealed' ? { step: 0 } : {}) }
    case 'trying': return { ...state, trying: action.move, ...(action.move ? { error: null } : {}) }
    case 'error': return { ...state, status: 'solving', error: action.error }
    case 'hint': return { ...state, hint: true }
    case 'seek': return { ...state, step: action.step }
    case 'type': return { ...state, typed: action.text, typedError: null }
    case 'typedError': return { ...state, typedError: action.text }
    case 'copy': return { ...state, copied: action.result }
  }
}

/** The API's lower time bound, using local midnight for today's games. */
export function practiceSince(range: PracticeRange, now: number): number | undefined {
  if (range === 'all') return undefined
  if (range === 'today') {
    const date = new Date(now * 1000)
    date.setHours(0, 0, 0, 0)
    return Math.floor(date.getTime() / 1000)
  }
  return now - Number(range) * DAY
}

/** A different continuation is accepted within this many winning-chance percentage points. */
const ALT_MARGIN = 5

export function alternativeLine(result: InteractiveResult, fen: string, uci: string, san: string, played: string[]): string[] | null {
  const side = (white: number) => fen.split(' ')[1] === 'w' ? white : 100 - white
  const best = result.lines[0]
  const mine = result.lines.find(line => line.uci[0] === uci)
  if (!best || !mine || !(side(best.win_white) - side(mine.win_white) <= ALT_MARGIN)) return null
  // The stored line no longer applies; keep the engine continuation after this move.
  return mine.san[0] === san ? [...played, ...mine.san] : [...played, san]
}

/** The puzzle as PGN: its position, the engine line and the move played in the game. */
export function puzzlePgn(item: PracticeItem, solution: PracticeResult, line: string[], username: string) {
  const chess = new Chess(item.fen)
  chess.setComment(`In the game: ${solution.game_san}. Best: ${solution.best_san}.`)
  for (const san of line) { try { chess.move(san) } catch { break } }
  const date = item.end_time ? new Date(item.end_time * 1000) : null
  const [white, black] = item.color === 'white' ? [username, item.opponent ?? '?'] : [item.opponent ?? '?', username]
  chess.setHeader('Event', 'Plyback practice')
  chess.setHeader('Date', date ? `${date.getFullYear()}.${String(date.getMonth() + 1).padStart(2, '0')}.${String(date.getDate()).padStart(2, '0')}` : '????.??.??')
  chess.setHeader('White', white)
  chess.setHeader('Black', black)
  return chess.pgn()
}
