import { niceTicks } from '../lib/charts'
import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'

export interface Series {
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

export function LineChart({ data, x, series, height = 220, yDomain, yUnit = '', xFormat, refY }:
  { data: Row[]; x: string; series: Series[]; height?: number; yDomain?: [number, number]; yUnit?: string
    xFormat?: (v: unknown) => string; refY?: number }) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const [hover, setHover] = useState<number | null>(null)
  const pad = { l: 40, r: 10, t: 8, b: 22 }
  const innerW = Math.max(10, width - pad.l - pad.r)
  const innerH = height - pad.t - pad.b
  const values = data.flatMap(d => series.map(s => num(d[s.key]))).filter((v): v is number => v !== null)
  const [lo, hi] = yDomain ?? [Math.min(...values), Math.max(...values)]
  const ticks = useMemo(() => niceTicks(lo, hi), [lo, hi])
  const y0 = yDomain ? lo : ticks[0]
  const y1 = yDomain ? hi : ticks[ticks.length - 1]
  const sx = (i: number) => pad.l + (data.length <= 1 ? innerW / 2 : (i / (data.length - 1)) * innerW)
  const sy = (v: number) => pad.t + innerH - ((v - y0) / (y1 - y0 || 1)) * innerH
  const fmtX = xFormat ?? ((v: unknown) => String(v))
  const xTickEvery = Math.max(1, Math.ceil(data.length / Math.max(2, Math.floor(innerW / 90))))

  const paths = series.map(s => {
    let d = ''
    let pen = false
    data.forEach((row, i) => {
      const v = num(row[s.key])
      if (v === null) { pen = false; return }
      d += `${pen ? 'L' : 'M'}${sx(i).toFixed(1)},${sy(v).toFixed(1)}`
      pen = true
    })
    return d
  })

  return (
    <div>
      <Legend series={series} />
      <div ref={ref} className="relative" style={{ height }}
        onMouseMove={e => {
          if (!data.length) return
          const r = e.currentTarget.getBoundingClientRect()
          const i = Math.round(((e.clientX - r.left - pad.l) / innerW) * (data.length - 1))
          setHover(Math.max(0, Math.min(data.length - 1, i)))
        }}
        onMouseLeave={() => setHover(null)}>
        {width > 0 && (
          <svg width={width} height={height} role="img" aria-label={`Chart of ${series.map(s => s.name).join(', ')}`}>
            {ticks.filter(t => t >= y0 && t <= y1).map(t => (
              <g key={t}>
                <line x1={pad.l} x2={width - pad.r} y1={sy(t)} y2={sy(t)} stroke="var(--grid)" />
                <text x={pad.l - 8} y={sy(t)} dy="0.32em" textAnchor="end" className="fill-fg-3 text-[10.5px] num">{t}{yUnit}</text>
              </g>
            ))}
            {refY !== undefined && <line x1={pad.l} x2={width - pad.r} y1={sy(refY)} y2={sy(refY)} stroke="var(--line-strong)" strokeDasharray="4 4" />}
            {data.map((row, i) => i % xTickEvery === 0 && (
              <text key={i} x={sx(i)} y={height - 6} textAnchor="middle" className="fill-fg-3 text-[10.5px]">{fmtX(row[x])}</text>
            ))}
            {series.map((s, k) => (
              <path key={s.key} d={paths[k]} fill="none" stroke={s.color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round"
                strokeDasharray={s.dashed ? '5 4' : undefined} opacity={s.dashed ? 0.85 : 1} />
            ))}
            {data.length < 32 && series.map(s => data.map((row, i) => {
              const v = num(row[s.key])
              return v === null ? null : <circle key={`${s.key}${i}`} cx={sx(i)} cy={sy(v)} r={2.5} fill={s.color} />
            }))}
            {hover !== null && hover < data.length && (
              <g>
                <line x1={sx(hover)} x2={sx(hover)} y1={pad.t} y2={pad.t + innerH} stroke="var(--line-strong)" />
                {series.map(s => {
                  const v = num(data[hover][s.key])
                  return v === null ? null : <circle key={s.key} cx={sx(hover)} cy={sy(v)} r={4} fill={s.color} stroke="var(--surface)" strokeWidth={2} />
                })}
              </g>
            )}
          </svg>
        )}
        {hover !== null && hover < data.length && width > 0 && (
          <Tip x={sx(hover)} y={Math.min(...series.map(s => { const v = num(data[hover][s.key]); return v === null ? height : sy(v) }))} width={width}>
            <div className="mb-0.5 font-medium text-fg">{fmtX(data[hover][x])}</div>
            {series.map(s => {
              const v = num(data[hover][s.key])
              return <div key={s.key} className="flex items-center gap-1.5 text-fg-2"><span className="h-2 w-2 rounded-full" style={{ background: s.color }} />{s.name}: <span className="num text-fg">{v === null ? '—' : `${+v.toFixed(1)}${yUnit}`}</span></div>
            })}
          </Tip>
        )}
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

