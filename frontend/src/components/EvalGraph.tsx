import { useMemo, useRef, useState } from 'react'
import { CLASS_META } from '../lib/classification'
import { formatEval } from '../lib/format'
import type { MoveRow } from '../lib/types'

interface Props {
  wins: number[]        // White win% per position (index 0 = start)
  evals: number[]       // White-POV cp per position
  moves: MoveRow[]
  current: number       // current ply (0 = start)
  onSelect: (ply: number) => void
  height?: number
}

const W = 1000

export default function EvalGraph({ wins, evals, moves, current, onSelect, height = 120 }: Props) {
  const [hover, setHover] = useState<number | null>(null)
  const ref = useRef<SVGSVGElement>(null)
  const n = Math.max(1, wins.length - 1)
  const H = height
  const x = (i: number) => (i / n) * W
  const y = (w: number) => H - (w / 100) * H

  const { area, line } = useMemo(() => {
    const pts = wins.map((w, i) => `${x(i).toFixed(1)},${y(w).toFixed(1)}`)
    return { area: `M0,${H} L${pts.join(' L')} L${W},${H} Z`, line: `M${pts.join(' L')}` }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wins, H])

  const markers = moves.filter(m => m.classification && ['blunder', 'mistake', 'miss', 'brilliant', 'great'].includes(m.classification))

  const plyFromEvent = (e: React.MouseEvent) => {
    const r = ref.current!.getBoundingClientRect()
    const px = ((e.clientX - r.left) / r.width) * W
    return Math.max(0, Math.min(n, Math.round((px / W) * n)))
  }

  const hp = hover ?? null
  return (
    <div className="relative select-none">
      <svg
        ref={ref} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="block h-[120px] w-full cursor-pointer rounded-lg"
        style={{ height }} role="img" aria-label="Evaluation graph"
        onMouseMove={e => setHover(plyFromEvent(e))} onMouseLeave={() => setHover(null)}
        onClick={e => onSelect(plyFromEvent(e))}
      >
        <rect width={W} height={H} fill="#3a3f47" />
        <path d={area} fill="#e9eaee" />
        <line x1={0} x2={W} y1={H / 2} y2={H / 2} stroke="#7d8590" strokeWidth={1} strokeDasharray="4 4" vectorEffect="non-scaling-stroke" />
        <path d={line} fill="none" stroke="#81b64c" strokeWidth={1.5} vectorEffect="non-scaling-stroke" opacity={0.9} />
        <line x1={x(current)} x2={x(current)} y1={0} y2={H} stroke="#81b64c" strokeWidth={2} vectorEffect="non-scaling-stroke" />
        {hp !== null && <line x1={x(hp)} x2={x(hp)} y1={0} y2={H} stroke="#fff" strokeOpacity={0.5} strokeWidth={1} vectorEffect="non-scaling-stroke" />}
      </svg>
      {/* markers rendered in HTML so they stay round regardless of aspect ratio */}
      <div className="pointer-events-none absolute inset-0">
        {markers.map(m => (
          <span key={m.ply} className="absolute h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 ring-[#1d2129]"
            style={{ left: `${(m.ply / n) * 100}%`, top: `${(1 - (wins[m.ply] ?? 50) / 100) * 100}%`, background: CLASS_META[m.classification!].color }} />
        ))}
      </div>
      {hp !== null && (
        <div className="pointer-events-none absolute -top-9 z-10 -translate-x-1/2 whitespace-nowrap rounded-md border border-line bg-surface px-2 py-1 text-xs shadow-lg"
          style={{ left: `${(hp / n) * 100}%` }}>
          {hp === 0 ? 'Start' : `${Math.ceil(hp / 2)}${hp % 2 ? '.' : '…'} ${moves[hp - 1]?.san ?? ''}`} · <span className="font-mono">{formatEval(evals[hp])}</span>
        </div>
      )}
    </div>
  )
}
