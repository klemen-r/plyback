export function niceTicks(min: number, max: number, count = 4): number[] {
  if (!isFinite(min) || !isFinite(max)) return [0, 1]
  if (min === max) { min -= 1; max += 1 }
  const span = max - min
  const step0 = span / count
  const mag = 10 ** Math.floor(Math.log10(step0))
  const step = [1, 2, 2.5, 5, 10].map(m => m * mag).find(s => span / s <= count) ?? step0
  const ticks = []
  for (let v = Math.floor(min / step) * step; v <= Math.ceil(max / step) * step + step * 0.001; v += step) ticks.push(+v.toFixed(10))
  return ticks
}

export const SERIES = {
  you: 'var(--accent-ink)',
  opp: 'var(--opp)',
  fg: 'var(--fg)',
  bad: 'var(--bad)',
  warn: 'var(--warn)',
}

/** A visible window over a series, in (fractional) point indices: [start, end]. */
export type View = [number, number]

/** Smallest window a zoom can reach: a few points, or the whole series if it is shorter. */
export function minSpan(n: number) {
  return Math.min(Math.max(1, n - 1), 8)
}

/** Keep a window inside [0, n - 1] without changing its width (unless it is wider than the data). */
export function clampView([a, b]: View, n: number): View {
  const last = Math.max(0, n - 1)
  const span = Math.min(Math.max(b - a, minSpan(n)), last)
  const start = Math.min(Math.max(0, a), last - span)
  return [start, start + span]
}

/** Zoom by `factor` (<1 zooms in) keeping the point at `anchor` (an index) where it is on screen. */
export function zoomView([a, b]: View, n: number, factor: number, anchor = (a + b) / 2): View {
  const span = Math.min(Math.max((b - a) * factor, minSpan(n)), Math.max(0, n - 1))
  const t = b > a ? (anchor - a) / (b - a) : 0.5
  return clampView([anchor - t * span, anchor - t * span + span], n)
}

/** Move a window by `delta` points. */
export function panView([a, b]: View, n: number, delta: number): View {
  return clampView([a + delta, b + delta], n)
}

export const isFullView = ([a, b]: View, n: number) => a <= 0.001 && b >= n - 1 - 0.001
