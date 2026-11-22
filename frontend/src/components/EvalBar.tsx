import { formatEval, shortEval, winWhite } from '../lib/format'

export default function EvalBar({ cp, orientation, className }: { cp: number | null | undefined; orientation: 'white' | 'black'; className?: string }) {
  const w = cp === null || cp === undefined ? 50 : winWhite(cp)
  const whiteOnBottom = orientation === 'white'
  const label = formatEval(cp)
  const whiteAhead = (cp ?? 0) >= 0
  return (
    <div className={`relative w-5 shrink-0 overflow-hidden rounded-[3px] bg-[#3a3a3a] ${className ?? ''}`} aria-label={`Evaluation ${label}`}>
      <div className="absolute inset-x-0 bg-[#f2f2f2] transition-all duration-300 ease-out"
        style={whiteOnBottom ? { bottom: 0, height: `${w}%` } : { top: 0, height: `${w}%` }} />
      <div className={`absolute inset-x-0 text-center text-[9.5px] font-semibold leading-none tnum ${whiteAhead ? 'text-[#3a3a3a]' : 'text-[#f2f2f2]'}`}
        style={(whiteAhead === whiteOnBottom) ? { bottom: 4 } : { top: 4 }}>
        {shortEval(cp)}
      </div>
    </div>
  )
}
