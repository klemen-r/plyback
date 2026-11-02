import { describe, expect, it } from 'vitest'
import { niceTicks } from '../charts'

describe('chart axes', () => {
  it('encloses the full data range to avoid clipping high points', () => {
    for (const [min, max] of [[0, 98], [802, 947], [-93, 43], [0.02, 0.097]]) {
      const ticks = niceTicks(min, max)
      expect(ticks[0]).toBeLessThanOrEqual(min)
      expect(ticks.at(-1)).toBeGreaterThanOrEqual(max)
    }
  })
  it('provides a finite, nonzero range for empty or constant data', () => {
    expect(niceTicks(Infinity, -Infinity)).toEqual([0, 1])
    const ticks = niceTicks(5, 5)
    expect(ticks[0]).toBeLessThan(5)
    expect(ticks.at(-1)).toBeGreaterThan(5)
  })
})
