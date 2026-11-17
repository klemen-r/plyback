import { Chess } from 'chess.js'
import { describe, expect, it } from 'vitest'
import { buildSteps, variationReducer } from '../variation'

const START = new Chess().fen()

describe('SAN steps', () => {
  it('keeps the legal prefix and records the actual board positions', () => {
    const steps = buildSteps(START, ['e4', 'e5', 'not a move', 'Nf3'])
    const chess = new Chess(); chess.move('e4'); chess.move('e5')
    expect(steps.map(s => s.san)).toEqual(['e4', 'e5'])
    expect(steps[1]).toEqual({ fen: chess.fen(), san: 'e5', from: 'e7', to: 'e5' })
    expect(buildSteps(START, [])).toEqual([])
  })
})

describe('variation reducer', () => {
  it('starts a variation and returns to the game', () => {
    const state = variationReducer(null, { type: 'start', fen: START, basePly: 8, sans: ['e4', 'e5'], source: 'engine' })!
    expect(state).toMatchObject({ baseFen: START, basePly: 8, idx: 1, source: 'engine' })
    expect(variationReducer(state, { type: 'exit' })).toBeNull()
    expect(variationReducer(state, { type: 'start', fen: START, basePly: 0, sans: ['illegal'], source: 'user' })).toBe(state)
  })

  it('branches from the cursor and keeps the variation origin', () => {
    const initial = variationReducer(null, { type: 'start', fen: START, basePly: 8, sans: ['e4', 'e5', 'Nf3'], source: 'engine' })!
    const cursor = variationReducer(initial, { type: 'seek', idx: 0 })!
    const next = variationReducer(cursor, { type: 'append', san: 'c5' })!
    expect(next.steps.map(s => s.san)).toEqual(['e4', 'c5'])
    expect(next).toMatchObject({ baseFen: START, basePly: 8, idx: 1, source: 'user' })
    expect(initial.steps.map(s => s.san)).toEqual(['e4', 'e5', 'Nf3'])
    const beforeFirst = variationReducer(initial, { type: 'seek', idx: -1 })!
    expect(variationReducer(beforeFirst, { type: 'append', san: 'd4' })!.steps.map(s => s.san)).toEqual(['d4'])
  })

  it('ignores invalid seeks and rejected moves without changing the cursor', () => {
    const state = variationReducer(null, { type: 'start', fen: START, basePly: 0, sans: ['e4'], source: 'user' })!
    expect(variationReducer(state, { type: 'seek', idx: -2 })).toBe(state)
    expect(variationReducer(state, { type: 'seek', idx: 1 })).toBe(state)
    expect(variationReducer(state, { type: 'append', san: 'e4' })).toBe(state)
    expect(variationReducer(null, { type: 'append', san: 'e4' })).toBeNull()
    expect(variationReducer(null, { type: 'seek', idx: 0 })).toBeNull()
  })

  it('continues engine lines from the displayed position and preserves the source', () => {
    const state = variationReducer(null, { type: 'start', fen: START, basePly: 0, sans: ['d4'], source: 'user' })!
    const next = variationReducer(state, { type: 'continueWithLine', from: state, fen: state.steps[0].fen, basePly: 7, sans: ['d5', 'c4'], idx: 1 })!
    expect(next.steps.map(s => s.san)).toEqual(['d4', 'd5', 'c4'])
    expect(next).toMatchObject({ baseFen: START, basePly: 0, idx: 2, source: 'user' })
    expect(variationReducer(state, { type: 'continueWithLine', from: null, fen: START, basePly: 5, sans: [], idx: 0 })).toBe(state)
    expect(variationReducer(state, { type: 'continueWithLine', from: null, fen: START, basePly: 5, sans: ['e4'], idx: 0 }))
      .toMatchObject({ baseFen: START, basePly: 5, idx: 0, source: 'engine' })
  })
})
