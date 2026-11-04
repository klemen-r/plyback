import clsx from 'clsx'
import { useEffect, useRef } from 'react'
import { CLASS_META } from '../lib/classification'
import type { MoveRow } from '../lib/types'
import ClassIcon from './ClassIcon'

const NOTABLE = new Set(['brilliant', 'great', 'inaccuracy', 'mistake', 'miss', 'blunder'])

export default function MoveList({ moves, current, onSelect, className = 'max-h-[340px]' }: { moves: MoveRow[]; current: number; onSelect: (ply: number) => void; className?: string }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current?.querySelector<HTMLElement>(`[data-ply="${current}"]`)
    if (el && ref.current) {
      const box = ref.current
      const top = el.offsetTop - box.offsetTop
      if (top < box.scrollTop || top > box.scrollTop + box.clientHeight - 40) box.scrollTo({ top: top - box.clientHeight / 2, behavior: 'smooth' })
    }
  }, [current])

  const startsWithBlack = moves[0]?.color === 'b'
  const rows: { num: number; w?: MoveRow; b?: MoveRow }[] = []
  moves.forEach((m, i) => {
    const num = Math.floor((i + (startsWithBlack ? 1 : 0)) / 2) + 1
    if (m.color === 'w' || rows.length === 0) rows.push({ num, w: m.color === 'w' ? m : undefined, b: m.color === 'b' ? m : undefined })
    else rows[rows.length - 1].b = m
  })

  const cell = (m?: MoveRow) => {
    if (!m) return <span />
    const c = m.classification
    const active = m.ply === current
    return (
      <button data-ply={m.ply} onClick={() => onSelect(m.ply)}
        className={clsx('flex items-center gap-1.5 rounded px-2 py-1 text-left font-mono text-[13px] transition-colors',
          active ? 'bg-hover text-fg' : 'text-fg/90 hover:bg-subtle')}>
        {c && NOTABLE.has(c) ? <ClassIcon c={c} size={15} /> : <span className="w-[15px]" />}
        <span style={c && NOTABLE.has(c) ? { color: CLASS_META[c].color } : undefined}>{m.san}</span>
      </button>
    )
  }

  return (
    <div ref={ref} className={clsx('overflow-y-auto pr-1', className)}>
      <div className="grid grid-cols-[2.25rem_1fr_1fr] gap-y-0.5">
        {rows.map(r => (
          <div key={r.num} className="contents">
            <span className="py-1 text-right pr-2 font-mono text-[12px] text-fg-3">{r.num}.</span>
            {cell(r.w)}
            {cell(r.b)}
          </div>
        ))}
      </div>
    </div>
  )
}
