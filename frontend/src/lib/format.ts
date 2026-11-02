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
  if (sec < 60) return `${Math.round(sec)}s`
  if (sec < 3600) return `${Math.floor(sec / 60)}m ${Math.round(sec % 60)}s`
  const h = Math.floor(sec / 3600)
  return `${h}h ${Math.floor((sec % 3600) / 60)}m`
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
  if (!tc) return '—'
  if (tc.includes('/')) {
    const days = Number(tc.split('/')[1]) / 86400
    return `${days} day${days === 1 ? '' : 's'}`
  }
  const [base, inc] = tc.split('+').map(Number)
  const b = base >= 60 ? `${base / 60}` : `${base}s`
  return inc ? `${b}+${inc}` : `${b} min`
}

export const pct = (v: number | null | undefined, digits = 0) => (v === null || v === undefined ? '—' : `${v.toFixed(digits)}%`)
export const num = (v: number | null | undefined, digits = 1) => (v === null || v === undefined ? '—' : v.toFixed(digits))

export function cap(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s
}

export const DAY = 86400
export function nowTs() { return Math.floor(Date.now() / 1000) }

export const moveLabel = (ply: number, san: string) => `${Math.ceil(ply / 2)}${ply % 2 ? '.' : '…'} ${san}`

