import { DAY, nowTs } from './format'
import type { InsightFilters } from './types'

export const TIME_CLASSES = ['bullet', 'blitz', 'rapid', 'daily'] as const

export interface FilterState {
  period: string
  time_classes: string[]
  color: '' | 'white' | 'black'
  rated: '' | 'true' | 'false'
  opening: string
  from?: string
  to?: string
}

export const DEFAULT_FILTERS: FilterState = { period: 'all', time_classes: [], color: '', rated: '', opening: '' }

export function restoreFilters(value: unknown): FilterState {
  const raw = value && typeof value === 'object' ? value as Record<string, unknown> : {}
  const timeClasses = Array.isArray(raw.time_classes) ? raw.time_classes : []
  const date = (v: unknown) => typeof v === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(v) ? v : undefined
  return {
    period: ['all', 'custom', '1', '7', '30', '90', '365'].includes(String(raw.period)) ? String(raw.period) : 'all',
    time_classes: TIME_CLASSES.filter(tc => timeClasses.includes(tc)),
    color: raw.color === 'white' || raw.color === 'black' ? raw.color : '',
    rated: raw.rated === 'true' || raw.rated === 'false' ? raw.rated : '',
    opening: typeof raw.opening === 'string' ? raw.opening.slice(0, 512) : '',
    from: date(raw.from), to: date(raw.to),
  }
}

export function toInsightFilters(s: FilterState): InsightFilters {
  const f: InsightFilters = {}
  if (s.period === 'custom') {
    const midnight = (date: string, nextDay = false) => {
      const [y, m, d] = date.split('-').map(Number)
      return Math.floor(new Date(y, m - 1, d + Number(nextDay)).getTime() / 1000)
    }
    // Dates entered the wrong way round mean the same range, rather than an error.
    const [from, to] = s.from && s.to && s.from > s.to ? [s.to, s.from] : [s.from, s.to]
    if (from) f.since = midnight(from)
    if (to) f.until = midnight(to, true)
  } else if (s.period !== 'all') {
    f.since = Math.floor((nowTs() - Number(s.period) * DAY) / 3600) * 3600 // hour-rounded: stable cache keys
  }
  if (s.time_classes.length) f.time_classes = s.time_classes
  if (s.color) f.color = s.color
  if (s.rated) f.rated = s.rated === 'true'
  if (s.opening) f.opening = s.opening
  return f
}

