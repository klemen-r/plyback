import type { DrawShape } from '@lichess-org/chessground/draw'
import { ArrowUpRight } from 'lucide-react'
import { memo } from 'react'
import { Link } from 'react-router-dom'
import { sanToSquares } from '../lib/chess'
import { fmtDate, moveLabel } from '../lib/format'
import type { Example } from '../lib/types'
import Board from './Board'
import ClassIcon from './ClassIcon'

export const MiniBoard = memo(function MiniBoard({ fen, orientation, played, best }: { fen: string; orientation: 'white' | 'black'; played?: string | null; best?: string | null }) {
  const shapes: DrawShape[] = []
  const p = sanToSquares(fen, played)
  const b = sanToSquares(fen, best)
  if (p) shapes.push({ orig: p[0], dest: p[1], brush: 'red' })
  if (b && (!p || b[0] !== p[0] || b[1] !== p[1])) shapes.push({ orig: b[0], dest: b[1], brush: 'green' })
  return <Board fen={fen} orientation={orientation} shapes={shapes} mini />
})

/** A concrete position from the player's games that supports a finding. */
export default function PositionCard({ ex }: { ex: Example }) {
  if (!ex.fen) return null
  return (
    <div className="group flex flex-col overflow-hidden rounded-lg border border-line bg-surface">
      <Link to={`/games/${ex.game_id}?ply=${ex.ply}`} className="block p-2.5 pb-0" aria-label={`Open ${moveLabel(ex.ply, ex.san, ex.fen)} in review`}>
        <MiniBoard fen={ex.fen} orientation={ex.color} played={ex.san} best={ex.best_san} />
      </Link>
      <div className="flex flex-1 flex-col gap-1 px-3 pb-3 pt-2.5">
        <div className="flex items-center gap-1.5 text-[13px]">
          <ClassIcon c={ex.classification} size={15} />
          <span className="font-mono font-medium">{moveLabel(ex.ply, ex.san, ex.fen)}</span>
          {ex.best_san && ex.best_san !== ex.san && <span className="truncate text-fg-3">· best <span className="font-mono text-fg-2">{ex.best_san}</span></span>}
        </div>
        {ex.explanation && <p className="line-clamp-3 text-[12.5px] leading-snug text-fg-2">{ex.explanation}</p>}
        <div className="mt-auto flex items-center justify-between gap-2 pt-1 text-[12px] text-fg-3">
          <span className="truncate">vs {ex.opponent} · {fmtDate(ex.end_time)}</span>
          {ex.lichess_url && (
            <a href={ex.lichess_url} target="_blank" rel="noreferrer" className="flex shrink-0 items-center gap-0.5 hover:text-fg" title="Open position on Lichess">
              Lichess<ArrowUpRight className="h-3 w-3" />
            </a>
          )}
        </div>
      </div>
    </div>
  )
}

export function Examples({ items, limit = 4 }: { items: Example[]; limit?: number }) {
  const shown = items.filter(e => e.fen).slice(0, limit)
  if (!shown.length) return <p className="text-[13px] text-fg-3">No examples in this selection.</p>
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4">
      {shown.map(e => <PositionCard key={`${e.game_id}-${e.ply}`} ex={e} />)}
    </div>
  )
}
