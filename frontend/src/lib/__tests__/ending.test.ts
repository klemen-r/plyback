import { describe, expect, it } from 'vitest'
import { endingOf } from '../ending'

const game = (white: string | null, black: string | null, user_color: 'white' | 'black' | null = 'white', termination: string | null = null) => ({
  white: { username: 'w', rating: null, result: white }, black: { username: 'b', rating: null, result: black }, user_color, termination,
})

describe('endingOf', () => {
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
