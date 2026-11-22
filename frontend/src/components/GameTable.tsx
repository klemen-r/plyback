import clsx from 'clsx'
import { useNavigate } from 'react-router-dom'
import { fmtDate, fmtTimeControl } from '../lib/format'
import type { Classification, GameSummary } from '../lib/types'
import ClassIcon from './ClassIcon'
import PlatformMark from './PlatformMark'
import { endingOf } from '../lib/ending'
import { ResultMark, Spinner } from './ui'

const SHOWN: Classification[] = ['brilliant', 'great', 'mistake', 'blunder']

function opponentOf(g: GameSummary) {
  return g.user_color === 'white' ? g.black : g.user_color === 'black' ? g.white : null
}

function ErrorCounts({ g }: { g: GameSummary }) {
  const c = g.user_color && g.counts ? g.counts[g.user_color] : null
  if (!c) return null
  return (
    <span className="inline-flex items-center gap-2">
      {SHOWN.map(k => (c[k] ?? 0) > 0 && (
        <span key={k} className="tnum inline-flex items-center gap-1 text-[12.5px] text-fg-2"><ClassIcon c={k} size={14} />{c[k]}</span>
      ))}
    </span>
  )
}

function AccuracyCell({ g }: { g: GameSummary }) {
  const mine = g.user_color ? g[g.user_color] : null
  if (g.analyzed && mine) return <span className="tnum">{mine.accuracy?.toFixed(1) ?? '—'}</span>
  if (g.job?.status === 'failed') return <span className="text-[12.5px] text-bad" title={g.job.error}>Failed</span>
  if (g.job) return <Spinner className="ml-auto h-3.5 w-3.5" />
  return <span className="text-fg-3">—</span>
}

/** Games as a table; rows open the review (or, with onSelect, select for a preview panel). */
export default function GameTable({ games, compact, selected, onSelect }:
  { games: GameSummary[]; compact?: boolean; selected?: number | null; onSelect?: (g: GameSummary) => void }) {
  const nav = useNavigate()
  const open = (g: GameSummary) => nav(`/games/${g.id}`)
  return (
    <table className="w-full text-[13.5px]">
      <thead>
        <tr className="border-b border-line text-left text-[12.5px] text-fg-3">
          <th className="w-8 py-2.5 pl-4 font-normal"><span className="sr-only">Result</span></th>
          <th className="py-2.5 pr-3 font-normal">Opponent</th>
          <th className={clsx('py-2.5 pr-3 font-normal', compact ? 'hidden xl:table-cell' : 'hidden sm:table-cell')}>Result</th>
          <th className={clsx('py-2.5 pr-3 font-normal', compact ? 'hidden lg:table-cell' : 'hidden md:table-cell')}>Opening</th>
          {!compact && <th className="hidden py-2.5 pr-3 font-normal sm:table-cell">Time</th>}
          {!compact && <th className="hidden py-2.5 pr-3 font-normal lg:table-cell">Notable moves</th>}
          <th className="py-2.5 pr-3 text-right font-normal">Accuracy</th>
          <th className="py-2.5 pr-4 text-right font-normal">Date</th>
        </tr>
      </thead>
      <tbody>
        {games.map(g => {
          const opp = opponentOf(g)
          return (
            <tr key={g.id} tabIndex={0} aria-selected={selected === g.id}
              onClick={() => (onSelect ? onSelect(g) : open(g))}
              onDoubleClick={() => open(g)}
              onKeyDown={e => { if (e.key === 'Enter') open(g); else if (e.key === ' ' && onSelect) { e.preventDefault(); onSelect(g) } }}
              className={clsx('cursor-pointer border-b border-line outline-none transition-colors last:border-0 focus-visible:bg-hover',
                selected === g.id ? 'bg-hover' : 'hover:bg-hover/60')}>
              <td className="py-2.5 pl-4"><ResultMark outcome={g.outcome} /></td>
              <td className="max-w-[220px] py-2.5 pr-3">
                <div className="flex items-center gap-2 truncate">
                  <PlatformMark platform={g.source} />
                  {g.user_color && <span className={clsx('h-2.5 w-2.5 shrink-0 rounded-[2px] border border-line-strong', g.user_color === 'white' ? 'bg-white' : 'bg-[#262626]')} title={`You played ${g.user_color}`} />}
                  <span className="truncate">{opp ? opp.username : `${g.white.username} – ${g.black.username}`}</span>
                  {opp?.rating && <span className="tnum text-fg-3">{opp.rating}</span>}
                </div>
              </td>
              <td className={clsx('whitespace-nowrap py-2.5 pr-3 text-fg-2', compact ? 'hidden xl:table-cell' : 'hidden sm:table-cell')}>{endingOf(g)?.text ?? '—'}</td>
              {/* In the compact table the opening takes whatever width is left and truncates, so the date stays visible. */}
              <td className={clsx('truncate py-2.5 pr-3 text-fg-2', compact ? 'hidden w-full max-w-0 lg:table-cell' : 'hidden max-w-[260px] md:table-cell')}>{g.opening.name ?? '—'}</td>
              {!compact && <td className="hidden whitespace-nowrap py-2.5 pr-3 text-fg-2 sm:table-cell">{fmtTimeControl(g.time_control)}</td>}
              {!compact && <td className="hidden py-2.5 pr-3 lg:table-cell"><ErrorCounts g={g} /></td>}
              <td className="py-2.5 pr-3 text-right"><AccuracyCell g={g} /></td>
              <td className="whitespace-nowrap py-2.5 pr-4 text-right text-fg-3">{fmtDate(g.end_time, false, true)}</td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}
