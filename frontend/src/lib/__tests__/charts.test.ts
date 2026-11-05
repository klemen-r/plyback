import { describe, expect, it } from 'vitest'
import { clampView, isFullView, minSpan, niceTicks, panView, zoomView } from '../charts'

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

describe('chart view window', () => {
  it('zooms around an anchor and never leaves the data', () => {
    expect(zoomView([0, 99], 100, 0.5, 0)).toEqual([0, 49.5])
    expect(zoomView([0, 99], 100, 0.5, 99)).toEqual([49.5, 99])
    const [a, b] = zoomView([0, 99], 100, 0.5, 49.5)
    expect([a, b]).toEqual([24.75, 74.25])
    // Zooming out past the data shows all of it.
    expect(zoomView([20, 40], 100, 10, 30)).toEqual([0, 99])
  })

  it('stops at a minimum width and keeps short series whole', () => {
    expect(zoomView([0, 99], 100, 0.001, 50)[1] - zoomView([0, 99], 100, 0.001, 50)[0]).toBe(minSpan(100))
    expect(zoomView([0, 3], 4, 0.1)).toEqual([0, 3])
  })

  it('pans within bounds and keeps the width', () => {
    expect(panView([10, 30], 100, 5)).toEqual([15, 35])
    expect(panView([10, 30], 100, -50)).toEqual([0, 20])
    expect(panView([10, 30], 100, 500)).toEqual([79, 99])
    expect(clampView([-5, 200], 100)).toEqual([0, 99])
    expect(isFullView([0, 99], 100)).toBe(true)
    expect(isFullView([0, 50], 100)).toBe(false)
  })
})
