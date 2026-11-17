import clsx from 'clsx'
import { Minus, Plus, RotateCcw } from 'lucide-react'
import type React from 'react'
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { clampView, isFullView, minSpan, niceTicks, panView, zoomView, type View } from '../lib/charts'

interface Series {
  key: string
  name: string
  color: string
  dashed?: boolean
}

type Row = Record<string, unknown>

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [w, setW] = useState(0)
  useLayoutEffect(() => {
    if (!ref.current) return
    // Measure straight away, so the first paint (and any early gesture) has the real width.
    setW(Math.floor(ref.current.getBoundingClientRect().width))
    const ro = new ResizeObserver(([e]) => setW(Math.floor(e.contentRect.width)))
    ro.observe(ref.current)
    return () => ro.disconnect()
  }, [])
  return [ref, w] as const
}

const num = (v: unknown) => (typeof v === 'number' && isFinite(v) ? v : null)

function Legend({ series }: { series: Series[] }) {
  if (series.length < 2) return null
  return (
    <div className="mb-2 flex flex-wrap gap-4 text-xs text-fg-2">
      {series.map(s => (
        <span key={s.key} className="inline-flex items-center gap-1.5">
          <span className="h-[3px] w-4 rounded-full" style={{ background: s.color, opacity: s.dashed ? 0.7 : 1 }} />{s.name}
        </span>
      ))}
    </div>
  )
}

function Tip({ x, y, width, children }: { x: number; y: number; width: number; children: ReactNode }) {
  const left = Math.min(Math.max(x, 70), width - 70)
  return (
    <div className="pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-full rounded-md border border-line bg-surface px-2.5 py-1.5 text-xs shadow-[var(--shadow-pop)]"
      style={{ left, top: y - 8 }}>{children}</div>
  )
}

export function LineChart({ data, x, series, height = 220, yDomain, yUnit = '', xFormat, refY, zoomable = false, unitLabel = 'points' }:
  { data: Row[]; x: string; series: Series[]; height?: number; yDomain?: [number, number]; yUnit?: string
    xFormat?: (v: unknown) => string; refY?: number
    /** Ctrl/⌘ + scroll or pinch to zoom, drag or scroll sideways to pan, double-click to reset. */
    zoomable?: boolean
    /** What a data point is, for the visible-range label ("12 of 80 games"). */
    unitLabel?: string }) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const clipId = useId().replace(/:/g, '')
  const [hover, setHover] = useState<number | null>(null)
  const n = data.length
  const full: View = useMemo(() => [0, Math.max(0, n - 1)], [n])
  // The zoom belongs to the data it was made on: new data (another period or account) shows everything.
  const [zoom, setZoom] = useState<{ data: Row[]; view: View } | null>(null)
  const view: View = zoom && zoom.data === data ? zoom.view : full
  const setView = (next: View | ((v: View) => View)) => setZoom(z => {
    const current = z && z.data === data ? z.view : full
    return { data, view: typeof next === 'function' ? next(current) : next }
  })
  const [a, b] = zoomable ? clampView(view, n) : full
  const zoomed = zoomable && !isFullView([a, b], n)

  const pad = { l: 40, r: 10, t: 8, b: 22 }
  const innerW = Math.max(10, width - pad.l - pad.r)
  const innerH = height - pad.t - pad.b
  const first = Math.max(0, Math.floor(a))
  const last = Math.min(n - 1, Math.ceil(b))
  const visible = data.slice(first, last + 1)
  const values = visible.flatMap(d => series.map(s => num(d[s.key]))).filter((v): v is number => v !== null)
  const [lo, hi] = yDomain ?? [Math.min(...values), Math.max(...values)]
  const ticks = useMemo(() => niceTicks(lo, hi), [lo, hi])
  const y0 = yDomain ? lo : ticks[0]
  const y1 = yDomain ? hi : ticks[ticks.length - 1]
  const sx = (i: number) => pad.l + (n <= 1 ? innerW / 2 : ((i - a) / (b - a || 1)) * innerW)
  const sy = (v: number) => pad.t + innerH - ((v - y0) / (y1 - y0 || 1)) * innerH
  const indexAt = (px: number) => a + ((px - pad.l) / innerW) * (b - a)
  const fmtX = xFormat ?? ((v: unknown) => String(v))
  const xTickEvery = Math.max(1, Math.ceil((last - first + 1) / Math.max(2, Math.floor(innerW / 90))))
  // Evenly spaced labels, skipping repeats (several games on one day share a date).
  const xTicks: [number, string][] = []
  for (let i = first; i <= last; i++) {
    if (i % xTickEvery !== 0) continue
    const label = fmtX(data[i][x])
    if (label !== xTicks.at(-1)?.[1]) xTicks.push([i, label])
  }

  const paths = series.map(s => {
    let d = ''
    let pen = false
    // One point either side of the window, so lines run to the edges (clipped below).
    for (let i = Math.max(0, first - 1); i <= Math.min(n - 1, last + 1); i++) {
      const v = num(data[i][s.key])
      if (v === null) { pen = false; continue }
      d += `${pen ? 'L' : 'M'}${sx(i).toFixed(1)},${sy(v).toFixed(1)}`
      pen = true
    }
    return d
  })

  // Gestures. Updates go through functional setState, so native listeners never use a stale view.
  const pointers = useRef(new Map<number, { x: number; y: number }>())
  const gesture = useRef<{ view: View; start: Map<number, number> } | null>(null)
  // The anchor is a position on screen, turned into an index against the latest view, so a burst of
  // wheel or pinch events within one frame zooms around the same point.
  const zoomAt = (factor: number, px?: number) => {
    const at = px === undefined ? 0.5 : Math.min(1, Math.max(0, (px - pad.l) / innerW))
    setView(v => {
      const [va, vb] = clampView(v, n)
      return zoomView([va, vb], n, factor, va + at * (vb - va))
    })
  }
  const panBy = (pixels: number) => setView(v => {
    const [va, vb] = clampView(v, n)
    return panView([va, vb], n, (pixels / innerW) * (vb - va))
  })

  useEffect(() => {
    const el = ref.current
    if (!zoomable || !el) return
    const onWheel = (e: WheelEvent) => {
      const r = el.getBoundingClientRect()
      if (e.ctrlKey || e.metaKey) {
        // Trackpad pinch arrives as ctrl + wheel with small deltas.
        e.preventDefault()
        zoomAt(Math.exp(Math.max(-0.5, Math.min(0.5, e.deltaY / 400))), e.clientX - r.left)
      } else if (zoomed && (Math.abs(e.deltaX) > Math.abs(e.deltaY) || (e.shiftKey && e.deltaY))) {
        // Sideways scrolling pans when zoomed; plain vertical scrolling keeps scrolling the page.
        e.preventDefault()
        panBy(e.deltaX || e.deltaY)
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  })

  // Drags and pinches are solved from where the gesture started: the games under the fingers stay
  // under the fingers, so nothing drifts however the events are batched.
  const latest = useRef<View>([a, b])
  useLayoutEffect(() => { latest.current = [a, b] })
  const begin = () => {
    gesture.current = pointers.current.size
      ? { view: latest.current, start: new Map([...pointers.current].map(([id, p]) => [id, p.x])) }
      : null
  }
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!zoomable || (e.pointerType === 'mouse' && e.button !== 0)) return
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY })
    begin()
    if (pointers.current.size > 1 || zoomed) { try { e.currentTarget.setPointerCapture(e.pointerId) } catch { /* pointer already gone */ } }
  }
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect()
    // A mouse released outside the chart (no capture while unzoomed) never sends pointerup here;
    // without this the stale press would block the tooltip until the next click.
    if (e.pointerType === 'mouse' && e.buttons === 0 && pointers.current.delete(e.pointerId)) begin()
    if (n && e.pointerType === 'mouse' && !gesture.current) setHover(Math.max(0, Math.min(n - 1, Math.round(indexAt(e.clientX - r.left)))))
    const g = gesture.current
    if (!zoomable || !g || !pointers.current.has(e.pointerId)) return
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY })
    const [va, vb] = g.view
    const toIndex = (px: number) => va + ((px - r.left - pad.l) / innerW) * (vb - va)
    const ids = [...g.start.keys()].filter(id => pointers.current.has(id))
    if (ids.length > 1) {
      const [p, q] = ids
      const i1 = toIndex(g.start.get(p)!), i2 = toIndex(g.start.get(q)!)
      const x1 = pointers.current.get(p)!.x - r.left - pad.l, x2 = pointers.current.get(q)!.x - r.left - pad.l
      // Fingers too close together or crossed over: keep the last view.
      if (Math.abs(x2 - x1) < 12 || (x2 - x1) * (i2 - i1) <= 0) return
      const span = ((i2 - i1) / (x2 - x1)) * innerW
      const start = i1 - (x1 / innerW) * span
      setView(clampView([start, start + span], n))
      setHover(null)
    } else if (ids.length === 1 && !isFullView(g.view, n)) {
      const dx = pointers.current.get(ids[0])!.x - g.start.get(ids[0])!
      setView(panView(g.view, n, -(dx / innerW) * (vb - va)))
      setHover(null)
    }
  }
  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    pointers.current.delete(e.pointerId)
    // A finger lifting mid-pinch continues as a drag from where the view is now.
    begin()
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!zoomable) return
    const step = (b - a) * 0.15
    const actions: Record<string, () => void> = {
      '+': () => zoomAt(0.7),
      '=': () => zoomAt(0.7),
      '-': () => zoomAt(1 / 0.7),
      '0': () => setView(full),
      ArrowLeft: () => setView(v => panView(clampView(v, n), n, -step)),
      ArrowRight: () => setView(v => panView(clampView(v, n), n, step)),
    }
    if (actions[e.key]) { e.preventDefault(); actions[e.key]() }
  }

  const tipIndex = hover !== null && hover >= first && hover <= last ? hover : null
  const iconButton = 'grid h-10 w-10 place-items-center text-fg-2 transition-colors hover:text-fg disabled:opacity-40 sm:h-7 sm:w-7'
  return (
    <div>
      <Legend series={series} />
      {zoomable && n > 2 && (
        <div className="mb-2 flex items-center gap-3 text-[12px] text-fg-3">
          <span className="min-w-0 truncate tnum" aria-live="polite">
            {zoomed ? `${fmtX(data[first][x])} – ${fmtX(data[last][x])} · ${last - first + 1} of ${n} ${unitLabel}` : `All ${n} ${unitLabel}`}
          </span>
          <span className="ml-auto hidden md:inline">Ctrl + scroll or pinch to zoom · drag to move</span>
          <div className="ml-auto flex shrink-0 items-center rounded-md border border-line md:ml-0">
            <button type="button" aria-label="Zoom out" disabled={!zoomed} onClick={() => zoomAt(1 / 0.6)} className={iconButton}>
              <Minus className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="Zoom in" disabled={b - a <= minSpan(n) + 0.001} onClick={() => zoomAt(0.6)} className={clsx(iconButton, 'border-x border-line')}>
              <Plus className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="Show everything" disabled={!zoomed} onClick={() => setView(full)} className={iconButton}>
              <RotateCcw className="h-3 w-3" />
            </button>
          </div>
        </div>
      )}
      <div ref={ref}
        className={clsx('relative', zoomable && 'select-none rounded outline-none focus-visible:ring-2 focus-visible:ring-accent-ink/40', zoomed && 'cursor-grab active:cursor-grabbing')}
        style={{ height, touchAction: zoomable ? 'pan-y' : undefined }}
        tabIndex={zoomable ? 0 : undefined}
        aria-label={zoomable ? 'Chart: + and − zoom, arrow keys move, 0 shows everything' : undefined}
        onKeyDown={onKeyDown}
        onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}
        onDoubleClick={() => { if (zoomable) setView(full) }}
        onMouseLeave={() => setHover(null)}>
        {width > 0 && (
          <svg width={width} height={height} role="img" aria-label={`Chart of ${series.map(s => s.name).join(', ')}`}>
            <defs><clipPath id={`clip-${clipId}`}><rect x={pad.l} y={0} width={innerW} height={height} /></clipPath></defs>
            {ticks.filter(t => t >= y0 && t <= y1).map(t => (
              <g key={t}>
                <line x1={pad.l} x2={width - pad.r} y1={sy(t)} y2={sy(t)} stroke="var(--grid)" />
                <text x={pad.l - 8} y={sy(t)} dy="0.32em" textAnchor="end" className="fill-fg-3 text-[10.5px] num">{t}{yUnit}</text>
              </g>
            ))}
            {refY !== undefined && <line x1={pad.l} x2={width - pad.r} y1={sy(refY)} y2={sy(refY)} stroke="var(--line-strong)" strokeDasharray="4 4" />}
            {xTicks.map(([i, label]) => (
              <text key={i} x={sx(i)} y={height - 6} textAnchor="middle" className="fill-fg-3 text-[10.5px]">{label}</text>
            ))}
            <g clipPath={`url(#clip-${clipId})`}>
              {series.map((s, k) => (
                <path key={s.key} d={paths[k]} fill="none" stroke={s.color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round"
                  strokeDasharray={s.dashed ? '5 4' : undefined} opacity={s.dashed ? 0.85 : 1} />
              ))}
              {visible.length < 32 && series.map(s => visible.map((row, k) => {
                const v = num(row[s.key])
                return v === null ? null : <circle key={`${s.key}${first + k}`} cx={sx(first + k)} cy={sy(v)} r={2.5} fill={s.color} />
              }))}
            </g>
            {tipIndex !== null && (
              <g>
                <line x1={sx(tipIndex)} x2={sx(tipIndex)} y1={pad.t} y2={pad.t + innerH} stroke="var(--line-strong)" />
                {series.map(s => {
                  const v = num(data[tipIndex][s.key])
                  return v === null ? null : <circle key={s.key} cx={sx(tipIndex)} cy={sy(v)} r={4} fill={s.color} stroke="var(--surface)" strokeWidth={2} />
                })}
              </g>
            )}
          </svg>
        )}
        {tipIndex !== null && width > 0 && (
          <Tip x={sx(tipIndex)} y={Math.min(...series.map(s => { const v = num(data[tipIndex][s.key]); return v === null ? height : sy(v) }))} width={width}>
            <div className="mb-0.5 font-medium text-fg">{fmtX(data[tipIndex][x])}</div>
            {series.map(s => {
              const v = num(data[tipIndex][s.key])
              return <div key={s.key} className="flex items-center gap-1.5 text-fg-2"><span className="h-2 w-2 rounded-full" style={{ background: s.color }} />{s.name}: <span className="num text-fg">{v === null ? '—' : `${+v.toFixed(1)}${yUnit}`}</span></div>
            })}
          </Tip>
        )}
      </div>
      {zoomed && <ScrollBar view={[a, b]} n={n} left={pad.l} right={pad.r} onChange={setView} />}
    </div>
  )
}

/** A slim scrollbar under a zoomed chart: drag the thumb, or press the track to jump there. */
function ScrollBar({ view: [a, b], n, left, right, onChange }: { view: View; n: number; left: number; right: number; onChange: (v: View) => void }) {
  const track = useRef<HTMLDivElement>(null)
  const drag = useRef<{ x: number; start: View } | null>(null)
  const last = n - 1
  const toIndex = (dx: number) => (dx / (track.current?.clientWidth || 1)) * last
  return (
    <div className="mt-2" style={{ paddingLeft: left, paddingRight: right }}>
      <div ref={track} className="relative h-2 cursor-pointer rounded-full bg-subtle" role="scrollbar" aria-orientation="horizontal"
        aria-valuemin={0} aria-valuemax={last} aria-valuenow={Math.round(a)} aria-label="Visible range"
        onPointerDown={e => {
          if (e.target !== e.currentTarget) return
          const centre = toIndex(e.clientX - e.currentTarget.getBoundingClientRect().left)
          onChange(clampView([centre - (b - a) / 2, centre + (b - a) / 2], n))
        }}>
        <div data-thumb className="absolute inset-y-0 cursor-grab touch-none rounded-full bg-line-strong transition-colors hover:bg-fg-3 active:cursor-grabbing"
          style={{ left: `${(a / last) * 100}%`, width: `${Math.max(2, ((b - a) / last) * 100)}%` }}
          onPointerDown={e => { try { e.currentTarget.setPointerCapture(e.pointerId) } catch { /* pointer already gone */ } drag.current = { x: e.clientX, start: [a, b] } }}
          onPointerMove={e => { const d = drag.current; if (d) onChange(panView(d.start, n, toIndex(e.clientX - d.x))) }}
          onPointerUp={() => { drag.current = null }}
          onPointerCancel={() => { drag.current = null }} />
      </div>
    </div>
  )
}

export function BarChart({ data, x, series, height = 200, unit = '', horizontal = false, labelWidth = 120, max }:
  { data: Row[]; x: string; series: Series[]; height?: number; unit?: string; horizontal?: boolean; labelWidth?: number; max?: number }) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const [hover, setHover] = useState<number | null>(null)
  const values = data.flatMap(d => series.map(s => num(d[s.key]) ?? 0))
  const top = max ?? niceTicks(0, Math.max(1, ...values)).slice(-1)[0]

  if (horizontal) {
    const rowH = Math.max(18, series.length * 10 + 10)
    return (
      <div>
        <Legend series={series} />
        <div className="space-y-2">
          {data.map((row, i) => (
            <div key={i} className="flex items-center gap-3 text-xs">
              <span className="shrink-0 truncate text-fg-2" style={{ width: labelWidth }} title={String(row[x])}>{String(row[x])}</span>
              <div className="flex flex-1 flex-col justify-center gap-[3px]" style={{ height: rowH }}>
                {series.map(s => {
                  const v = num(row[s.key]) ?? 0
                  return (
                    <div key={s.key} className="flex items-center gap-2">
                      <div className="h-[7px] rounded-full" style={{ width: `${Math.max(0.5, (100 * v) / (top || 1))}%`, background: s.color }} />
                      <span className="num text-[11px] text-fg-2">{+v.toFixed(1)}{unit}</span>
                    </div>
                  )
                })}
              </div>
            </div>
          ))}
        </div>
      </div>
    )
  }

  const pad = { l: 40, r: 6, t: 8, b: 22 }
  const innerW = Math.max(10, width - pad.l - pad.r)
  const innerH = height - pad.t - pad.b
  const band = innerW / Math.max(1, data.length)
  const barW = Math.min(28, (band * 0.7) / series.length)
  const ticks = niceTicks(0, top)
  const sy = (v: number) => pad.t + innerH - (v / (top || 1)) * innerH
  return (
    <div>
      <Legend series={series} />
      <div ref={ref} className="relative" style={{ height }} onMouseLeave={() => setHover(null)}>
        {width > 0 && (
          <svg width={width} height={height} role="img" aria-label={`Bar chart of ${series.map(s => s.name).join(', ')}`}>
            {ticks.filter(t => t <= top).map(t => (
              <g key={t}>
                <line x1={pad.l} x2={width - pad.r} y1={sy(t)} y2={sy(t)} stroke="var(--grid)" />
                <text x={pad.l - 8} y={sy(t)} dy="0.32em" textAnchor="end" className="fill-fg-3 text-[10.5px] num">{t}{unit}</text>
              </g>
            ))}
            {data.map((row, i) => {
              const cx = pad.l + band * i + band / 2
              const groupW = barW * series.length + 2 * (series.length - 1)
              return (
                <g key={i} onMouseEnter={() => setHover(i)}>
                  <rect x={pad.l + band * i} y={pad.t} width={band} height={innerH} fill={hover === i ? 'var(--hover)' : 'transparent'} />
                  {series.map((s, k) => {
                    const v = num(row[s.key])
                    if (v === null) return null
                    return <rect key={s.key} x={cx - groupW / 2 + k * (barW + 2)} y={sy(v)} width={barW} height={Math.max(0, sy(0) - sy(v))} rx={3} fill={s.color} />
                  })}
                  <text x={cx} y={height - 6} textAnchor="middle" className="fill-fg-3 text-[10.5px]">{String(row[x])}</text>
                </g>
              )
            })}
          </svg>
        )}
        {hover !== null && hover < data.length && width > 0 && (
          <Tip x={pad.l + band * hover + band / 2} y={pad.t + 10} width={width}>
            <div className="mb-0.5 font-medium text-fg">{String(data[hover][x])}</div>
            {series.map(s => {
              const v = num(data[hover][s.key])
              return <div key={s.key} className="flex items-center gap-1.5 text-fg-2"><span className="h-2 w-2 rounded-full" style={{ background: s.color }} />{s.name}: <span className="num text-fg">{v === null ? '—' : `${+v.toFixed(1)}${unit}`}</span></div>
            })}
          </Tip>
        )}
      </div>
    </div>
  )
}

/** Win / draw / loss proportion bar. */
export function WDLBar({ wins, draws, losses, className }: { wins: number; draws: number; losses: number; className?: string }) {
  const n = wins + draws + losses || 1
  return (
    <div className={`flex h-1.5 w-full overflow-hidden rounded-full bg-subtle ${className ?? ''}`} title={`${wins} won · ${draws} drawn · ${losses} lost`}>
      <div className="bg-good" style={{ width: `${(100 * wins) / n}%` }} />
      <div className="bg-fg-3" style={{ width: `${(100 * draws) / n}%` }} />
      <div className="bg-bad" style={{ width: `${(100 * losses) / n}%` }} />
    </div>
  )
}

