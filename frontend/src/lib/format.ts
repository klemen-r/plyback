const MATE_CP = 100_000
const MATE_THRESHOLD = MATE_CP - 1000

export function mateIn(cp: number): number | null {
  if (Math.abs(cp) < MATE_THRESHOLD) return null
  const n = MATE_CP - Math.abs(cp)
  return cp > 0 ? n : -n
}

/** Evaluation from White's point of view: "+1.25", "M3", "-M2", "1-0". */
export function formatEval(cp: number | undefined | null): string {
  if (cp === undefined || cp === null) return '—'
  const m = mateIn(cp)
  if (m !== null) {
    if (m === 0) return cp > 0 ? '1-0' : '0-1'
    return m > 0 ? `M${m}` : `-M${-m}`
  }
  const v = cp / 100
  return `${v > 0 ? '+' : ''}${v.toFixed(Math.abs(v) >= 10 ? 1 : 2)}`
}

export function winWhite(cp: number): number {
  if (Math.abs(cp) >= MATE_THRESHOLD) return cp > 0 ? 100 : 0
  const c = Math.max(-1000, Math.min(1000, cp))
  return 50 + 50 * (2 / (1 + Math.exp(-0.00368208 * c)) - 1)
}

export function fmtDate(ts: number | null | undefined, withTime = false, short = false): string {
  if (!ts) return '—'
  const d = new Date(ts * 1000)
  const sameYear = d.getFullYear() === new Date().getFullYear()
  if (short) return d.toLocaleDateString(undefined, sameYear ? { month: 'short', day: 'numeric' } : { month: 'short', year: 'numeric' })
  return withTime
    ? d.toLocaleString(undefined, { month: 'short', day: 'numeric', year: 'numeric', hour: '2-digit', minute: '2-digit' })
    : d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
}

export function timeAgo(ts: number | null | undefined): string {
  if (!ts) return 'never'
  const s = Math.max(0, Date.now() / 1000 - ts)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} d ago`
  return fmtDate(ts)
}

export function fmtDuration(sec: number | null | undefined): string {
  if (sec === null || sec === undefined) return '—'
  // Round once up front so 59.6 s reads 1m 0s, not 60s, and 119.6 s reads 2m 0s, not 1m 60s.
  const s = Math.round(sec)
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`
}

export function fmtClock(sec: number | null | undefined): string {
  if (sec === null || sec === undefined) return ''
  if (sec >= 3600) {
    const h = Math.floor(sec / 3600)
    const m = Math.floor((sec % 3600) / 60)
    return `${h}:${String(m).padStart(2, '0')}:${String(Math.floor(sec % 60)).padStart(2, '0')}`
  }
  const m = Math.floor(sec / 60)
  const s = sec % 60
  return sec < 20 ? `${m}:${s.toFixed(1).padStart(4, '0')}` : `${m}:${String(Math.floor(s)).padStart(2, '0')}`
}

export function fmtTimeControl(tc: string | null | undefined): string {
  if (!tc || tc === '?') return '—'
  // PGN "-" means no time limit (e.g. Chess.com daily games against a coach bot).
  if (tc === '-') return 'Untimed'
  const daily = /^1\/(\d+)$/.exec(tc)
  if (daily) {
    const days = Number(daily[1]) / 86400
    return `${+days.toFixed(2)} day${days === 1 ? '' : 's'}`
  }
  const live = /^(\d+)(?:\+(\d+(?:\.\d+)?))?$/.exec(tc)
  // Other PGN forms (e.g. "40/7200:3600") are shown as written.
  if (!live) return tc
  const base = Number(live[1])
  const inc = Number(live[2] ?? 0)
  const b = base >= 60 ? `${+(base / 60).toFixed(2)}` : `${base}s`
  return inc ? `${b}+${inc}` : `${b} min`
}

export const pct = (v: number | null | undefined, digits = 0) => (v === null || v === undefined ? '—' : `${v.toFixed(digits)}%`)
export const num = (v: number | null | undefined, digits = 1) => (v === null || v === undefined ? '—' : v.toFixed(digits))

export function cap(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s
}

export const DAY = 86400
export function nowTs() { return Math.floor(Date.now() / 1000) }

/** The move number from a position's FEN (games can start from a set-up position), else `fallback`. */
export function moveNumber(fen: string | null | undefined, fallback: number) {
  const n = Number(fen?.split(' ')[5])
  return Number.isInteger(n) && n > 0 ? n : fallback
}

/**
 * "42… Kh7". With the position before the move, the number and side come from its FEN,
 * so games imported from a set-up position keep their numbering; otherwise from the ply.
 */
export function moveLabel(ply: number, san: string, fen?: string | null) {
  const [, side, , , , full] = fen?.split(' ') ?? []
  const n = Number(full)
  if ((side === 'w' || side === 'b') && Number.isInteger(n) && n > 0) return `${n}${side === 'b' ? '…' : '.'} ${san}`
  return `${Math.ceil(ply / 2)}${ply % 2 ? '.' : '…'} ${san}`
}

