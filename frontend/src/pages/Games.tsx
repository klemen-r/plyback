import { useQuery } from '@tanstack/react-query'
import { ArrowRight, ChevronLeft, ChevronRight, Search, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import ClassIcon from '../components/ClassIcon'
import { MiniBoard } from '../components/Evidence'
import GameTable from '../components/GameTable'
import { Button, Card, Empty, ErrorNote, Loading, PageHeader, Select } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api, type GameQuery } from '../lib/api'
import { CLASS_META } from '../lib/classification'
import { fmtDate, fmtTimeControl, moveLabel } from '../lib/format'
import type { GameSummary } from '../lib/types'

const PAGE = 30

/** Preview of the selected game: its costliest moment on a board, plus the summary. */
function Preview({ g, onClose }: { g: GameSummary; onClose: () => void }) {
  const { account } = useAccount()
  const detail = useQuery({ queryKey: ['game', g.id, account?.id], queryFn: () => api.game(g.id, account?.id) })
  const me = g.user_color
  const moves = detail.data?.moves ?? []
  const mine = me ? moves.filter(m => m.color === me[0]) : moves
  const worst = useMemo(() => [...mine].filter(m => m.ep_loss !== undefined).sort((a, b) => (b.ep_loss ?? 0) - (a.ep_loss ?? 0))[0], [mine])
  const opp = me === 'white' ? g.black : me === 'black' ? g.white : null
  return (
    <Card padded={false} className="sticky top-32 overflow-hidden">
      <div className="flex items-start justify-between gap-2 border-b border-line px-4 py-3">
        <div className="min-w-0">
          <div className="truncate font-medium">{opp ? `vs ${opp.username}` : `${g.white.username} – ${g.black.username}`}</div>
          <div className="text-[12.5px] text-fg-3">{fmtDate(g.end_time, true)} · {fmtTimeControl(g.time_control)} · {g.result}</div>
        </div>
        <Button size="icon" variant="ghost" onClick={onClose} aria-label="Close preview"><X className="h-4 w-4" /></Button>
      </div>
      <div className="p-4">
        {detail.isLoading ? <div className="skeleton aspect-square w-full" /> : worst && worst.classification ? (
          <>
            <MiniBoard fen={worst.fen_before} orientation={me ?? 'white'} played={worst.san} best={worst.best_san} />
            <div className="mt-3 flex items-center gap-1.5 text-[13.5px] font-medium">
              <ClassIcon c={worst.classification} size={15} />{moveLabel(worst.ply, worst.san)}
              <span className="font-normal text-fg-2">— {CLASS_META[worst.classification].label.toLowerCase()}</span>
            </div>
            {worst.explanation && <p className="mt-1 text-[13px] text-fg-2">{worst.explanation}</p>}
          </>
        ) : detail.data?.moves.length ? (
          <MiniBoard fen={detail.data.game.final_fen ?? detail.data.moves[0].fen_before} orientation={me ?? 'white'} />
        ) : null}
        <dl className="mt-4 grid grid-cols-2 gap-3 text-[13px]">
          <div><dt className="text-fg-3">Opening</dt><dd className="truncate">{g.opening.name ?? '—'}</dd></div>
          <div><dt className="text-fg-3">Your accuracy</dt><dd className="tnum">{me && g[me].accuracy !== null ? g[me].accuracy?.toFixed(1) : '—'}</dd></div>
        </dl>
        <Link to={`/games/${g.id}${worst ? `?ply=${worst.ply}` : ''}`} className="mt-4 flex h-8 items-center justify-center gap-1.5 rounded-md bg-accent text-[13.5px] font-medium text-accent-fg hover:opacity-90">
          Open review <ArrowRight className="h-3.5 w-3.5" />
        </Link>
      </div>
    </Card>
  )
}

export default function Games() {
  const { account } = useAccount()
  const [sp, setSp] = useSearchParams()
  const [search, setSearch] = useState(sp.get('q') ?? '')
  const [selected, setSelected] = useState<GameSummary | null>(null)
  const get = (k: string) => sp.get(k) ?? ''
  const set = (k: string, v: string) => {
    const next = new URLSearchParams(sp)
    if (v) next.set(k, v); else next.delete(k)
    if (k !== 'page') next.delete('page')
    setSp(next, { replace: true })
  }
  useEffect(() => {
    const t = setTimeout(() => { if (search !== get('q')) set('q', search) }, 300)
    return () => clearTimeout(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search])

  const page = Number(get('page') || 1)
  const query: GameQuery = {
    account_id: account?.id, page, page_size: PAGE, time_class: get('time_class') || undefined, outcome: get('outcome') || undefined,
    color: get('color') || undefined, opening: get('opening') || undefined, q: get('q') || undefined,
    analyzed: get('analyzed') ? get('analyzed') === '1' : undefined, classification: get('classification') || undefined,
    sort: get('sort') || undefined,
  }
  const q = useQuery({ queryKey: ['games', query], queryFn: () => api.games(query), placeholderData: p => p, refetchInterval: 15000 })
  const openings = useQuery({ queryKey: ['openings', account?.id], queryFn: () => api.openings(account?.id) })
  const pages = q.data ? Math.max(1, Math.ceil(q.data.total / PAGE)) : 1
  const wide = typeof window !== 'undefined' && window.matchMedia('(min-width: 1280px)').matches

  return (
    <div className="appear">
      <PageHeader title="Games" description={q.data ? `${q.data.total.toLocaleString()} games${account ? ` played by ${account.display_name}` : ''}` : ' '}
        actions={<Link to="/import"><Button>Import PGN</Button></Link>} />
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-2 h-4 w-4 text-fg-3" />
          <input value={search} onChange={e => setSearch(e.target.value)} placeholder="Opponent or opening" aria-label="Search games"
            className="h-8 w-56 rounded-md border border-line bg-surface pl-8 pr-2.5 text-[13.5px] outline-none placeholder:text-fg-3 hover:border-line-strong focus:border-accent-ink" />
        </div>
        <Select label="Time control" value={get('time_class')} onChange={v => set('time_class', v)}>
          <option value="">All time controls</option><option value="bullet">Bullet</option><option value="blitz">Blitz</option><option value="rapid">Rapid</option><option value="daily">Daily</option>
        </Select>
        {account && <>
          <Select label="Result" value={get('outcome')} onChange={v => set('outcome', v)}>
            <option value="">All results</option><option value="win">Wins</option><option value="loss">Losses</option><option value="draw">Draws</option>
          </Select>
          <Select label="Colour" value={get('color')} onChange={v => set('color', v)}>
            <option value="">Both colours</option><option value="white">White</option><option value="black">Black</option>
          </Select>
          <Select label="Contains" value={get('classification')} onChange={v => set('classification', v)}>
            <option value="">Any moves</option>
            {(['brilliant', 'great', 'blunder', 'miss'] as const).map(c => <option key={c} value={c}>With a {CLASS_META[c].label.toLowerCase()}</option>)}
          </Select>
        </>}
        <Select label="Opening" value={get('opening')} onChange={v => set('opening', v)} className="max-w-[220px]">
          <option value="">All openings</option>
          {openings.data?.map(o => <option key={o.family} value={o.family}>{o.family} ({o.n})</option>)}
        </Select>
        <Select label="Review status" value={get('analyzed')} onChange={v => set('analyzed', v)}>
          <option value="">Reviewed or not</option><option value="1">Reviewed</option><option value="0">Not reviewed</option>
        </Select>
        <Select label="Sort" value={get('sort')} onChange={v => set('sort', v)}>
          <option value="">Newest first</option><option value="end_time_asc">Oldest first</option><option value="accuracy_desc">Highest accuracy</option><option value="accuracy_asc">Lowest accuracy</option>
        </Select>
      </div>
      <div className={selected && wide ? 'grid grid-cols-[minmax(0,1fr)_340px] items-start gap-6' : ''}>
        <div>
          <Card padded={false} className="overflow-hidden">
            {q.isLoading ? <Loading /> : q.error ? <div className="p-4"><ErrorNote error={q.error} /></div> : q.data?.items.length
              ? <GameTable games={q.data.items} selected={selected?.id} onSelect={wide ? g => setSelected(s => (s?.id === g.id ? null : g)) : undefined} />
              : <Empty title="No games match these filters">Clear some filters, or connect an account to import games.</Empty>}
          </Card>
          {pages > 1 && (
            <div className="mt-4 flex items-center justify-between text-[13px]">
              <span className="tnum text-fg-3">Page {page} of {pages}</span>
              <div className="flex gap-2">
                <Button size="sm" disabled={page <= 1} onClick={() => set('page', String(page - 1))}><ChevronLeft className="h-3.5 w-3.5" />Previous</Button>
                <Button size="sm" disabled={page >= pages} onClick={() => set('page', String(page + 1))}>Next<ChevronRight className="h-3.5 w-3.5" /></Button>
              </div>
            </div>
          )}
          {wide && !selected && q.data?.items.length ? <p className="mt-3 text-[12.5px] text-fg-3">Select a game to preview it; double-click or press Enter to open the review.</p> : null}
        </div>
        {selected && wide && <Preview g={selected} onClose={() => setSelected(null)} />}
      </div>
    </div>
  )
}
