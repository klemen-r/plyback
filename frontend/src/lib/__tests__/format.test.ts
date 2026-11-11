import { describe, expect, it } from 'vitest'
import { dueIn, fmtClock, fmtDuration, fmtTimeControl, formatEval, mateIn, moveLabel, moveNumber, winWhite } from '../format'
import { restoreFilters, toInsightFilters, type FilterState } from '../filters'

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
    expect(fmtTimeControl('1/259200')).toBe('3 days')
    expect(fmtTimeControl('-')).toBe('Untimed')
    expect(fmtTimeControl('?')).toBe('—')
    expect(fmtTimeControl('40/7200:3600')).toBe('40/7200:3600')
    expect(fmtTimeControl('90+0.5')).toBe('1.5+0.5')
  })
  it('rounds durations before splitting them into units', () => {
    expect(fmtDuration(59.6)).toBe('1m 0s')
    expect(fmtDuration(119.6)).toBe('2m 0s')
    expect(fmtDuration(3599.7)).toBe('1h 0m')
    expect(fmtDuration(42.4)).toBe('42s')
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

describe('moveLabel', () => {
  it('numbers from the ply for games from the start', () => {
    expect(moveLabel(1, 'e4')).toBe('1. e4')
    expect(moveLabel(4, 'Nf6')).toBe('2… Nf6')
  })
  it('numbers from the FEN for games imported from a set-up position', () => {
    expect(moveLabel(1, 'Kh7', '7k/8/8/8/8/8/8/7K b - - 0 42')).toBe('42… Kh7')
    expect(moveLabel(2, 'Kh2', '8/7k/8/8/8/8/8/7K w - - 1 43')).toBe('43. Kh2')
    expect(moveLabel(3, 'Kg8', 'not a fen')).toBe('2. Kg8')
  })
})

describe('custom date ranges', () => {
  it('treats dates entered the wrong way round as the same range', () => {
    const base: Omit<FilterState, 'from' | 'to'> = { period: 'custom', time_classes: [], color: '', rated: '', opening: '' }
    expect(toInsightFilters({ ...base, from: '2026-10-28', to: '2026-10-25' }))
      .toEqual(toInsightFilters({ ...base, from: '2026-10-25', to: '2026-10-28' }))
  })
})

describe('moveNumber', () => {
  it('reads the FEN move number, else falls back', () => {
    expect(moveNumber('7k/8/8/8/8/8/8/7K b - - 0 42', 1)).toBe(42)
    expect(moveNumber(null, 7)).toBe(7)
    expect(moveNumber('bad', 3)).toBe(3)
  })
})

describe('practice due wording', () => {
  // Local times, so the test holds in any time zone.
  const at = (day: number, hour: number, minute = 0) => Math.floor(new Date(2026, 10, day, hour, minute).getTime() / 1000)
  it('names the day a position is due, and never calls a same-day time tomorrow', () => {
    expect(dueIn(at(10, 0), at(9, 9))).toBe('tomorrow')
    expect(dueIn(at(10, 0), at(9, 21))).toBe('tomorrow')
    expect(dueIn(at(10, 0), at(9, 23, 50))).toBe('tomorrow')
    expect(dueIn(at(12, 0), at(9, 9))).toBe('in 3 days')
    // Due tonight (an older schedule, or another time zone's day): say when, not "tomorrow".
    expect(dueIn(at(9, 21), at(9, 9))).toBe('in 12h 0m')
    expect(dueIn(at(9, 9, 10), at(9, 9))).toBe('later in this session')
    expect(dueIn(at(9, 8), at(9, 9))).toBe('later in this session')
  })
})
