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
