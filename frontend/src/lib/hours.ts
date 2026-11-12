import type { Insights } from './types'

/**
 * Score by local hour of day. Each 15-minute bucket goes to the local hour that applied on its
 * own date, so summer-time games land in the right hour. Older saved reports only have UTC hours;
 * those use today's offset.
 */
export function localHours(r: Pick<Insights['results'], 'by_hour_utc' | 'by_quarter_hour'>) {
  const games = Array<number>(24).fill(0)
  const points = Array<number>(24).fill(0)
  const scores: (number | null)[] = Array(24).fill(null)
  if (r.by_quarter_hour) {
    for (const [index, n, p] of r.by_quarter_hour) {
      const hour = new Date(index * 900_000).getHours()
      games[hour] += n
      points[hour] += p
    }
    games.forEach((n, h) => { scores[h] = n ? Math.round(1000 * points[h] / n) / 10 : null })
  } else {
    const offset = Math.round(-new Date().getTimezoneOffset() / 60)
    r.by_hour_utc.forEach(b => { const h = (b.hour_utc + offset + 24) % 24; games[h] = b.games; scores[h] = b.score })
  }
  return games.map((n, h) => ({ hour: `${h}:00`, games: n, score: n >= 5 ? scores[h] : null }))
}
