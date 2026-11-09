import { describe, expect, it } from 'vitest'
import { gameOverText, isGameOverAnswer } from '../chess'
import { difference, knownAccount } from '../compare'
import { clockBeforeFirstMove, startingClock } from '../format'
import { buildSteps, playEngineLine, type Variation } from '../variation'

const START = 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1'

describe('engine lines in a variation', () => {
  it('continue the variation from the step shown instead of replacing it', () => {
    const steps = buildSteps(START, ['e4', 'e5', 'Nf3'])
    const variation: Variation = { baseFen: START, basePly: 0, steps, idx: 1, source: 'user' }
    // The live engine analyses the position after 1. e4 e5 and the user clicks its second move.
    const next = playEngineLine(variation, steps[1].fen, 7, ['Nc3', 'Nf6', 'f4'], 1)!
    expect(next.steps.map(s => s.san)).toEqual(['e4', 'e5', 'Nc3', 'Nf6', 'f4'])
    expect(next.idx).toBe(3)
    expect(next.baseFen).toBe(START)
    expect(next.basePly).toBe(0)
  })

  it('start a new variation from the game position outside one', () => {
    const next = playEngineLine(null, START, 4, ['d4', 'd5'], 5)!
    expect(next).toMatchObject({ baseFen: START, basePly: 4, idx: 1, source: 'engine' })
    expect(next.steps.map(s => s.san)).toEqual(['d4', 'd5'])
    expect(playEngineLine(null, START, 0, [], 0)).toBeNull()
  })
})

describe('finished positions in the live engine', () => {
  it('recognises the terminal answer the server sends', () => {
    expect(isGameOverAnswer([{ san: [], depth: 0 }])).toBe(true)
    expect(isGameOverAnswer([{ san: ['e4'], depth: 18 }])).toBe(false)
    expect(isGameOverAnswer([])).toBe(false)
  })

  it('names how the game ended and who won', () => {
    // Fool's mate: White to move and mated.
    expect(gameOverText('rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3')).toBe('Checkmate. Black wins.')
    expect(gameOverText('7k/5Q2/6K1/8/8/8/8/8 b - - 0 1')).toBe('Stalemate. The game is drawn.')
    expect(gameOverText('7k/8/8/8/8/8/8/K7 w - - 0 1')).toBe('Drawn by insufficient material.')
    expect(gameOverText('7k/8/8/8/8/8/8/KR6 w - - 150 120')).toBe('Drawn by the 75-move rule.')
  })
})

describe('clocks before the first move', () => {
  it('read the base time from live time controls only', () => {
    expect(startingClock('180+2')).toBe(180)
    expect(startingClock('600')).toBe(600)
    expect(startingClock('1/86400')).toBeNull()
    expect(startingClock('-')).toBeNull()
    expect(startingClock(null)).toBeNull()
  })

  it('show the base time until that side has moved, when the game records clocks', () => {
    const moves = [{ color: 'w' as const, clock: 178 }, { color: 'b' as const, clock: 179 }]
    expect(clockBeforeFirstMove(moves, 0, 'white', '180+2')).toBe(180)
    expect(clockBeforeFirstMove(moves, 1, 'black', '180+2')).toBe(180)
    expect(clockBeforeFirstMove(moves, 1, 'white', '180+2')).toBeNull()
    expect(clockBeforeFirstMove([{ color: 'w' as const, clock: null }], 0, 'white', '180+2')).toBeNull()
    expect(clockBeforeFirstMove(moves, 0, 'white', '1/86400')).toBeNull()
  })
})

describe('Compare', () => {
  it('colours a difference by its displayed value', () => {
    expect(difference(50.04, 50, 1, 'higher', '%')).toEqual({ text: '0.0%', tone: 'none' })
    expect(difference(50, 50.04, 1, 'higher', '%')).toEqual({ text: '0.0%', tone: 'none' })
    expect(difference(52, 50, 1, 'higher', '%')).toEqual({ text: '+2.0%', tone: 'good' })
    expect(difference(52, 50, 1, 'lower')).toEqual({ text: '+2.0', tone: 'bad' })
    expect(difference(null, 50, 1, 'higher')).toEqual({ text: '—', tone: 'none' })
  })

  it('drops an account id that is not connected', () => {
    expect(knownAccount(2, [{ id: 1 }, { id: 2 }])).toBe(2)
    expect(knownAccount(99, [{ id: 1 }])).toBeNull()
    expect(knownAccount(null, [{ id: 1 }])).toBeNull()
  })
})
