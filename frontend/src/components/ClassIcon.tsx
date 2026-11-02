import clsx from 'clsx'
import { CLASS_META } from '../lib/classification'
import type { Classification } from '../lib/types'

export default function ClassIcon({ c, size = 18, className }: { c: Classification; size?: number; className?: string }) {
  const m = CLASS_META[c]
  return (
    <span
      title={`${m.label}: ${m.description}`}
      className={clsx('inline-flex shrink-0 items-center justify-center rounded-full font-extrabold text-white', className)}
      style={{ width: size, height: size, background: m.color, fontSize: m.symbol.length > 1 ? size * 0.5 : size * 0.6, lineHeight: 1 }}
    >
      {m.symbol}
    </span>
  )
}

export function ClassLabel({ c }: { c: Classification }) {
  const m = CLASS_META[c]
  return (
    <span className="inline-flex items-center gap-1.5 font-semibold" style={{ color: m.color }}>
      <ClassIcon c={c} size={20} /> {m.label}
    </span>
  )
}
