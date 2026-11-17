import { describe, expect, it } from 'vitest'
import { endingOf, endShapes, outcomeOf } from '../ending'

const game = (white: string | null, black: string | null, user_color: 'white' | 'black' | null = 'white', termination: string | null = null) => ({
  white: { username: 'w', rating: null, result: white }, black: { username: 'b', rating: null, result: black }, user_color, termination,
})

describe('endingOf', () => {
  it('words Lichess statuses that have no result code', () => {
    expect(endingOf(game('win', 'lose', 'white', 'cheat'))).toEqual({ short: 'White won', text: 'Cheat detected' })
    expect(endingOf(game(null, null, 'white', 'unknownFinish'))).toEqual({ short: 'Unknown finish', text: 'Unknown finish' })
  })

  it('describes decisive endings from your side', () => {
    expect(endingOf(game('win', 'checkmated'))).toEqual({ short: 'Checkmate', text: 'Won by checkmate' })
    expect(endingOf(game('win', 'checkmated', 'black'))).toEqual({ short: 'Checkmate', text: 'Lost by checkmate' })
    expect(endingOf(game('timeout', 'win'))).toEqual({ short: 'Time', text: 'Lost on time' })
    expect(endingOf(game('timeout', 'win', 'black'))).toEqual({ short: 'Time', text: 'Won on time' })
    expect(endingOf(game('resigned', 'win'))).toEqual({ short: 'Resignation', text: 'You resigned' })
    expect(endingOf(game('win', 'resigned'))).toEqual({ short: 'Resignation', text: 'Won by resignation' })
    expect(endingOf(game('win', 'abandoned'))).toEqual({ short: 'Abandonment', text: 'Won, opponent left' })
  })

  it('describes draws the same way for both sides', () => {
    expect(endingOf(game('repetition', 'repetition'))).toEqual({ short: 'Repetition', text: 'Draw by repetition' })
    expect(endingOf(game('agreed', 'agreed', 'black'))).toEqual({ short: 'Agreement', text: 'Draw by agreement' })
    expect(endingOf(game('stalemate', 'stalemate'))?.text).toBe('Draw by stalemate')
    expect(endingOf(game('timevsinsufficient', 'timevsinsufficient'))?.short).toBe('Time vs material')
    expect(endingOf(game('50move', '50move'))?.text).toBe('Draw by the 50-move rule')
  })

  it('names the winning colour without an account, and falls back to stored text', () => {
    expect(endingOf(game('win', 'resigned', null))?.text).toBe('White won by resignation')
    expect(endingOf(game(null, null, null, 'Game drawn by repetition'))?.text).toBe('Game drawn by repetition')
    expect(endingOf(game(null, null, null))).toBeNull()
  })
})

describe('a win without a known reason', () => {
  it('says who won instead of "won by loss"', () => {
    expect(endingOf(game('win', 'lose'))).toEqual({ short: 'White won', text: 'Won' })
    expect(endingOf(game('win', 'lose', 'black'))).toEqual({ short: 'White won', text: 'Lost' })
    expect(endingOf(game('lose', 'win', null))).toEqual({ short: 'Black won', text: 'Black won' })
    expect(endingOf(game('win', 'lose', 'white', 'White wins by adjudication'))?.text).toBe('White wins by adjudication')
  })
})

const result = (white: string | null, black: string | null, score = '*') => ({
  white: { result: white }, black: { result: black }, result: score,
})
const START = 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1'
const FOOLS_MATE = 'rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3'
const STALEMATE = '7k/5Q2/6K1/8/8/8/8/8 b - - 0 1'

describe('outcomeOf', () => {
  it('reads checkmate and stalemate from the final position, even without result codes', () => {
    expect(outcomeOf(result(null, null), FOOLS_MATE)).toEqual({ white: 'mate', black: 'win' })
    expect(outcomeOf(result(null, null), STALEMATE)).toEqual({ white: 'draw', black: 'draw' })
  })
  it('uses the stored result for resignation, time, draws and plain scores', () => {
    expect(outcomeOf(result('resigned', 'win'), START)).toEqual({ white: 'resign', black: 'win' })
    expect(outcomeOf(result('win', 'timeout'), START)).toEqual({ white: 'win', black: 'time' })
    expect(outcomeOf(result('agreed', 'agreed'), START)).toEqual({ white: 'draw', black: 'draw' })
    expect(outcomeOf(result('timevsinsufficient', 'timeout'), START)).toEqual({ white: 'draw', black: 'draw' })
    expect(outcomeOf(result(null, null, '1-0'), START)).toEqual({ white: 'win', black: 'loss' })
    expect(outcomeOf(result(null, null, '1/2-1/2'), START)).toEqual({ white: 'draw', black: 'draw' })
    expect(outcomeOf(result(null, null, '*'), START)).toBeNull()
  })
})

describe('endShapes', () => {
  it('puts a badge on each king', () => {
    expect(endShapes(result(null, null), FOOLS_MATE).map(s => s.orig)).toEqual(['e1', 'e8'])
    expect(endShapes(result(null, null), STALEMATE).map(s => s.orig)).toEqual(['g6', 'h8'])
    expect(endShapes(result(null, null), START)).toEqual([])
  })
})
