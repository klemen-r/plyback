import { describe, expect, it } from 'vitest'
import { fmtClock, fmtTimeControl, formatEval, mateIn, winWhite } from '../format'
import { restoreFilters, toInsightFilters } from '../filters'

describe('format helpers', () => {
  it('formats evaluations and mates like the backend', () => {
    expect(formatEval(125)).toBe('+1.25')
    expect(formatEval(-40)).toBe('-0.40')
    expect(formatEval(100_000 - 3)).toBe('M3')
    expect(formatEval(-(100_000 - 2))).toBe('-M2')
    expect(formatEval(100_000)).toBe('1-0')
    expect(mateIn(500)).toBeNull()
  })
  it('win% is symmetric and saturates', () => {
    expect(winWhite(0)).toBeCloseTo(50)
    expect(winWhite(300) + winWhite(-300)).toBeCloseTo(100)
    expect(winWhite(100_000 - 1)).toBe(100)
  })
  it('formats clocks and time controls', () => {
    expect(fmtClock(185)).toBe('3:05')
    expect(fmtClock(9.4)).toBe('0:09.4')
    expect(fmtTimeControl('180+2')).toBe('3+2')
    expect(fmtTimeControl('60')).toBe('1 min')
    expect(fmtTimeControl('1/86400')).toBe('1 day')
  })
  it('converts filter state to API filters', () => {
    const f = toInsightFilters({ period: 'all', time_classes: ['blitz'], color: 'white', rated: 'true', opening: '' })
    expect(f).toEqual({ time_classes: ['blitz'], color: 'white', rated: true })
    const g = toInsightFilters({ period: '30', time_classes: [], color: '', rated: '', opening: '' })
    expect(g.since).toBeGreaterThan(0)
  })
  it('keeps short periods to the last day or week, hour-rounded', () => {
    const now = Math.floor(Date.now() / 1000)
    for (const [period, days] of [['1', 1], ['7', 7]] as const) {
      const since = toInsightFilters({ period, time_classes: [], color: '', rated: '', opening: '' }).since!
      expect(since % 3600).toBe(0)
      expect(now - since).toBeGreaterThanOrEqual(days * 86400)
      expect(now - since).toBeLessThan(days * 86400 + 3600)
      expect(restoreFilters({ period }).period).toBe(period)
    }
  })
  it('interprets custom dates in local time with the next local midnight as the upper bound', () => {
    const f = toInsightFilters({ period: 'custom', from: '2026-10-25', to: '2026-10-25', time_classes: [], color: '', rated: '', opening: '' })
    expect(f.since).toBe(new Date(2026, 9, 25).getTime() / 1000)
    expect(f.until).toBe(new Date(2026, 9, 26).getTime() / 1000)
  })
  it('recovers safely from outdated or damaged saved filters', () => {
    const f = restoreFilters({ time_classes: null, period: {}, color: 2, opening: false })
    expect(toInsightFilters(f)).toEqual({})
    expect(restoreFilters({ time_classes: ['blitz', 'not-a-time-class', 'blitz'] }).time_classes).toEqual(['blitz'])
  })
})
