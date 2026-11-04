import { ArrowRight } from 'lucide-react'
import { Link } from 'react-router-dom'
import { useCostliestMove } from '../lib/costliest'
import { CLASS_META, ERROR_CLASSES } from '../lib/classification'
import { fmtDate, fmtTimeControl, moveLabel } from '../lib/format'
import type { GameSummary } from '../lib/types'
import ClassIcon from './ClassIcon'
import { MiniBoard } from './Evidence'
import { ResultMark } from './ui'

function reviewState(g: GameSummary) {
  if (g.pgn_error) return `This game can't be reviewed: ${g.pgn_error}`
  if (!['chess', 'chess960'].includes(g.rules)) return 'Variant games are not reviewed.'
  if (g.job?.status === 'running') return 'Being reviewed now…'
  if (g.job?.status === 'queued') return 'Queued for review.'
  if (g.job?.status === 'failed') return 'Review failed; open it to retry.'
  return 'Not reviewed yet.'
}

/** Compact view of the most recent game: its costliest moment on a small board, beside the summary. */
export default function LatestGame({ g }: { g: GameSummary }) {
  const { detail, worst } = useCostliestMove(g)
  const me = g.user_color
  const opp = me === 'white' ? g.black : me === 'black' ? g.white : null
  const mine = me && g.counts ? g.counts[me] : null
  const errors = mine ? ERROR_CLASSES.filter(c => (mine[c] ?? 0) > 0) : []
  const fen = worst?.fen_before ?? detail.data?.game.final_fen ?? null
  const href = `/games/${g.id}${worst ? `?ply=${worst.ply}` : ''}`
  return (
    <div className="grid grid-cols-[112px_minmax(0,1fr)] gap-4 p-4 sm:grid-cols-[148px_minmax(0,1fr)]">
      <Link to={href} aria-label="Open the latest game in review" className="block self-start">
        {fen ? <MiniBoard fen={fen} orientation={me ?? 'white'} played={worst?.san} best={worst?.best_san} /> : <div className="skeleton aspect-square w-full" />}
      </Link>
      <div className="flex min-w-0 flex-col">
        <div className="flex items-center gap-2">
          <ResultMark outcome={g.outcome} />
          <span className="truncate font-medium">{opp ? `vs ${opp.username}` : `${g.white.username} – ${g.black.username}`}</span>
          {opp?.rating && <span className="shrink-0 text-[13px] text-fg-3 tnum">{opp.rating}</span>}
        </div>
        <div className="mt-0.5 truncate text-[12.5px] text-fg-3">{fmtDate(g.end_time, true)} · {fmtTimeControl(g.time_control)} {g.time_class} · {g.opening.name ?? 'Unknown opening'}</div>

        {g.analyzed ? (
          <>
            <dl className="mt-3 flex flex-wrap gap-x-6 gap-y-2 text-[13px]">
              {me && <div><dt className="text-fg-3">Your accuracy</dt><dd className="text-[18px] font-semibold leading-tight tracking-[-0.02em] tnum">{g[me].accuracy?.toFixed(1) ?? '—'}</dd></div>}
              {opp && <div><dt className="text-fg-3">Opponent</dt><dd className="text-[18px] font-semibold leading-tight tracking-[-0.02em] tnum text-fg-2">{opp.accuracy?.toFixed(1) ?? '—'}</dd></div>}
              {errors.length > 0 && (
                <div><dt className="text-fg-3">Your errors</dt>
                  <dd className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-1">
                    {errors.map(c => <span key={c} className="flex items-center gap-1 tnum" title={CLASS_META[c].label}><ClassIcon c={c} size={14} />{mine![c]}</span>)}
                  </dd>
                </div>
              )}
            </dl>
            {worst && worst.classification && (
              <p className="mt-3 line-clamp-3 text-[13px] text-fg-2 max-sm:hidden">
                <span className="font-medium text-fg">Costliest moment: {moveLabel(worst.ply, worst.san)}</span>
                {worst.explanation ? ` — ${worst.explanation}` : ''}
              </p>
            )}
          </>
        ) : (
          <p className="mt-3 text-[13px] text-fg-2">{reviewState(g)}</p>
        )}

        <Link to={href} className="mt-3 inline-flex h-10 w-fit items-center gap-1.5 rounded-md bg-accent px-3 text-[13.5px] font-medium text-accent-fg transition-opacity hover:opacity-90 sm:mt-auto sm:h-8">
          Open review <ArrowRight className="h-3.5 w-3.5" />
        </Link>
      </div>
    </div>
  )
}
