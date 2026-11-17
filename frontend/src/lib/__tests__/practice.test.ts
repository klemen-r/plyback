import { describe, expect, it } from 'vitest'
import { Chess } from 'chess.js'
import { alternativeLine, initialPuzzleState, practiceReducer, practiceSince, puzzlePgn } from '../practice'
import { dueIn } from '../format'
import type { InteractiveResult, PracticeItem, PracticeResult } from '../types'

const START = new Chess().fen()
const item: PracticeItem = {
  game_id: 1, ply: 1, fen: START, color: 'white', move_number: 1, classification: 'mistake',
  phase: 'opening', ep_loss: 0.2, themes: ['fork'], last_uci: null, last_san: null,
  end_time: null, time_class: 'rapid', opponent: 'Opponent', opponent_rating: null,
  status: 'new', box: 0, attempts: 0, correct: 0,
}
const result: PracticeResult = {
  correct: true, uci: 'e2e4', san: 'e4', best_uci: 'e2e4', best_san: 'e4', themes: ['fork'],
  line_san: ['e4', 'e5', 'Nf3'], game_san: 'a3', explanation: null, box: 1, due_at: 600,
  stats: { total: 1, new: 0, due: 0, learning: 1, mastered: 0, next_due_at: null },
}
const engineLine = (uci: string, san: string[], win: number) => ({ cp: 0, eval: '0', depth: 14, win_white: win, uci: [uci], san })

describe('practice scheduling and export', () => {
  it('uses local midnight for today and exact rolling bounds for longer ranges', () => {
    const date = new Date(2026, 10, 13, 15, 30)
    const now = Math.floor(date.getTime() / 1000)
    expect(practiceSince('all', now)).toBeUndefined()
    expect(practiceSince('today', now)).toBe(new Date(2026, 10, 13).getTime() / 1000)
    expect(practiceSince('7', now)).toBe(now - 7 * 86400)
    expect(practiceSince('30', now)).toBe(now - 30 * 86400)
  })

  it('labels the next review relative to an explicit clock', () => {
    const now = new Date(2026, 10, 13, 10).getTime() / 1000
    expect(dueIn(now + 600, now)).toBe('later in this session')
    expect(dueIn(now + 86400, now)).toBe('tomorrow')
    expect(dueIn(now + 3 * 86400, now)).toBe('in 3 days')
  })

  it('exports the position, names, comment and legal prefix of the solution', () => {
    const pgn = puzzlePgn(item, result, ['e4', 'e5', 'not a move', 'Nf3'], 'Player')
    expect(pgn).toContain('[Event "Plyback practice"]')
    expect(pgn).toContain('[Date "????.??.??"]')
    expect(pgn).toContain('[White "Player"]')
    expect(pgn).toContain('[Black "Opponent"]')
    expect(pgn).toContain('{In the game: a3. Best: e4.} 1. e4 e5')
    expect(pgn).not.toContain('Nf3')
    const setup = { ...item, color: 'black' as const, fen: '4k3/1P6/8/8/8/8/8/4K3 w - - 0 42',
      end_time: new Date(2026, 10, 13).getTime() / 1000, opponent: null }
    const fromSetup = puzzlePgn(setup, result, ['b8=N'], 'Player')
    expect(fromSetup).toContain('[SetUp "1"]')
    expect(fromSetup).toContain(`[FEN "${setup.fen}"]`)
    expect(fromSetup).toContain('[White "?"]')
    expect(fromSetup).toContain('[Black "Player"]')
    expect(fromSetup).toContain('[Date "2026.11.13"]')
    expect(fromSetup).toContain('42. b8=N')
  })
})

describe('alternative continuations', () => {
  it('compares winning chance for the side to move and accepts the exact margin', () => {
    const response: InteractiveResult = { fen: START, lines: [engineLine('g1f3', ['Nf3'], 60), engineLine('f1c4', ['Bc4', 'Nf6', 'd3'], 55)] }
    expect(alternativeLine(response, START, 'f1c4', 'Bc4', ['e4', 'e5'])).toEqual(['e4', 'e5', 'Bc4', 'Nf6', 'd3'])
    response.lines[1].win_white = 54.9
    expect(alternativeLine(response, START, 'f1c4', 'Bc4', [])).toBeNull()
    response.lines[1].win_white = Number.NaN
    expect(alternativeLine(response, START, 'f1c4', 'Bc4', [])).toBeNull()
    response.lines[0].win_white = 40
    response.lines[1].win_white = 45
    expect(alternativeLine(response, START.replace(' w ', ' b '), 'f1c4', 'Bc4', [])).toEqual(['Bc4', 'Nf6', 'd3'])
  })

  it('requires the played move in the engine lines and keeps no mismatched SAN continuation', () => {
    const response = { fen: START, lines: [engineLine('e2e4', ['e4'], 55)] }
    expect(alternativeLine(response, START, 'd2d4', 'd4', [])).toBeNull()
    expect(alternativeLine({ fen: START, lines: [] }, START, 'e2e4', 'e4', [])).toBeNull()
    expect(alternativeLine(response, START, 'e2e4', 'e4+', ['Nf3'])).toEqual(['Nf3', 'e4+'])
  })
})

describe('one practice puzzle', () => {
  it('starts fresh and freezes line settings once an answer has been attempted', () => {
    const initial = initialPuzzleState(true)
    expect(initial).toMatchObject({ status: 'solving', solution: null, played: [], tries: 0, lineFor: true })
    const off = practiceReducer(initial, { type: 'setting', fullLine: false })
    expect(off.lineFor).toBe(false)
    const answered = practiceReducer(off, { type: 'answer', result })
    expect(practiceReducer(answered, { type: 'setting', fullLine: true })).toBe(answered)
    const played = practiceReducer(initial, { type: 'play', moves: ['e4'] })
    expect(practiceReducer(played, { type: 'setting', fullLine: false })).toBe(played)
    expect(initialPuzzleState(true)).not.toBe(initial)
    expect(initial.solution).toBeNull()
  })

  it('clears retry feedback and hints on an accepted move, then records engine alternatives', () => {
    let state = practiceReducer(initialPuzzleState(true), { type: 'reject', san: 'a3' })
    state = practiceReducer(state, { type: 'reject', san: 'h3' })
    state = practiceReducer(state, { type: 'hint' })
    expect(state).toMatchObject({ wrong: 'h3', tries: 2, hint: true })
    state = practiceReducer(state, { type: 'play', moves: ['e4'] })
    expect(state).toMatchObject({ wrong: null, tries: 2, hint: false, played: ['e4'] })
    state = practiceReducer(state, { type: 'checking' })
    expect(state.status).toBe('checking')
    state = practiceReducer(state, { type: 'alternative', line: ['e4', 'e5', 'Bc4'], note: 'Works too' })
    expect(state).toMatchObject({ status: 'solving', liveLine: ['e4', 'e5', 'Bc4'], note: 'Works too' })
  })

  it('restores a failed check without charging a wrong try and reveals from the initial position', () => {
    let state = practiceReducer(initialPuzzleState(true), { type: 'trying', move: { fen: START, from: 'f1', to: 'c4' } })
    state = practiceReducer(state, { type: 'error', error: new Error('Timed out') })
    expect(state).toMatchObject({ status: 'solving', tries: 0 })
    expect(state.error).toBeInstanceOf(Error)
    state = practiceReducer(state, { type: 'finish', status: 'revealed' })
    expect(state).toMatchObject({ status: 'revealed', step: 0, trying: null, error: null })
    expect(practiceReducer(state, { type: 'seek', step: 2 }).step).toBe(2)
    expect(practiceReducer(state, { type: 'finish', status: 'solved' }).status).toBe('solved')
  })

  it('keeps typed validation and copying local to the puzzle', () => {
    let state = practiceReducer(initialPuzzleState(false), { type: 'typedError', text: 'Illegal move' })
    state = practiceReducer(state, { type: 'type', text: 'Nf3' })
    expect(state).toMatchObject({ typed: 'Nf3', typedError: null })
    expect(practiceReducer(state, { type: 'copy', result: 'yes' }).copied).toBe('yes')
    state = practiceReducer(state, { type: 'trying', move: null })
    expect(state.trying).toBeNull()
  })
})
