import type { Key } from '@lichess-org/chessground/types'
import { useQuery } from '@tanstack/react-query'
import { Chess } from 'chess.js'
import clsx from 'clsx'
import { ChevronLeft, RotateCcw } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import Board from '../components/Board'
import { WDLBar } from '../components/charts'
import { Button, Card, Empty, ErrorNote, Loading, PageHeader, Select, Tabs } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { pct } from '../lib/format'
import type { RepertoireNode } from '../lib/types'

type Side = 'white' | 'black'
const TIME_CLASSES = [['all', 'All time controls'], ['bullet', 'Bullet'], ['blitz', 'Blitz'], ['rapid', 'Rapid'], ['daily', 'Daily']] as const

/** "1. e4 e5 2. Nf3" from a list of SAN moves starting at ply 1. */
function numbered(moves: string[]) {
  return moves.map((san, i) => (i % 2 === 0 ? `${i / 2 + 1}. ${san}` : san)).join(' ')
}

function position(moves: string[]) {
  const chess = new Chess()
  let last: [Key, Key] | null = null
  for (const san of moves) {
    try { const m = chess.move(san); last = [m.from as Key, m.to as Key] } catch { break }
  }
  return { fen: chess.fen(), last }
}

/** What went wrong with your move in these games, most serious first. */
function errorSummary(n: RepertoireNode) {
  if (n.blunders) return `blunder in ${n.blunders} of ${n.games}`
  if (n.misses) return `missed chance in ${n.misses} of ${n.games}`
  if (n.errors) return `inaccuracy or mistake in ${n.errors} of ${n.games}`
  return null
}

/** Score colour only means something with a few games behind it. */
function scoreTone(n: RepertoireNode) {
  if (n.games < 3) return 'text-fg-2'
  return n.score >= 0.55 ? 'text-good' : n.score <= 0.45 ? 'text-bad' : 'text-fg'
}

function MoveRow({ node, focus, opening, merged, onClick }: { node: RepertoireNode; focus: boolean; opening: string | null; merged: boolean; onClick: () => void }) {
  const errorRate = node.mine && node.errors ? node.errors / node.games : 0
  const notes = [
    node.book === node.games ? 'Book' : node.left_book ? `Leaves book${node.left_book < node.games ? ` in ${node.left_book} of ${node.games}` : ''}` : null,
    node.opening && node.opening !== opening ? node.opening : null,
    merged && node.orders > 1 ? `${node.orders} move orders` : null,
  ].filter(Boolean)
  return (
    <button type="button" onClick={onClick}
      className={clsx('grid w-full grid-cols-[4rem_2.75rem_minmax(0,1fr)_3rem] items-center gap-x-3 px-4 py-2.5 text-left transition-colors hover:bg-hover/60',
        focus && 'bg-hover/60')}>
      <span className="font-mono text-[14px] font-semibold">{node.san}</span>
      <span className="tnum text-right text-[13px] text-fg-2">{node.games}</span>
      <WDLBar wins={node.wins} draws={node.draws} losses={node.losses} />
      <span className={clsx('tnum text-right text-[13px] font-medium', scoreTone(node))}>{pct(node.score * 100)}</span>
      {(notes.length > 0 || errorRate > 0) && (
        <span className="col-span-4 mt-1 truncate text-[12.5px] text-fg-2">
          {notes.join(' · ')}
          {errorRate > 0 && <span className={errorRate >= 0.5 ? 'text-bad' : undefined}>{notes.length ? ' · ' : ''}{(s => s[0].toUpperCase() + s.slice(1))(errorSummary(node)!)} games</span>}
        </span>
      )}
    </button>
  )
}

export default function Openings() {
  const { account, loading } = useAccount()
  const [params, setParams] = useSearchParams()
  const color: Side = params.get('color') === 'black' ? 'black' : 'white'
  // An unknown time control in the URL means all of them, as the select shows.
  const timeClass = TIME_CLASSES.find(([v]) => v === params.get('tc'))?.[0] ?? 'all'
  const plies = params.get('plies') === '30' ? 30 : 16
  const merge = params.get('merge') === '1'
  const requested = useMemo(() => (params.get('line') ?? '').split(' ').filter(Boolean), [params])
  const focus = params.get('focus')
  const [notice, setNotice] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  // Moves replace the URL instead of adding history entries, so browser back leaves the page
  // in one step (use the back arrow, ← or Backspace to step back a move).
  const update = (patch: Record<string, string | null>) => {
    const next = new URLSearchParams(params)
    for (const [k, v] of Object.entries(patch)) { if (v === null || v === '') next.delete(k); else next.set(k, v) }
    setNotice(null)
    setParams(next, { replace: true })
  }
  const go = (moves: string[], highlight: string | null = null) => update({ line: moves.join(' ') || null, focus: highlight })

  const q = useQuery({
    queryKey: ['repertoire', account?.id, color, timeClass, plies, merge],
    queryFn: () => api.repertoire(account!.id, { color, time_class: timeClass === 'all' ? undefined : timeClass, plies, merge: merge || undefined }),
    enabled: !!account,
  })
  // Moves are edges between positions; with merged move orders, several lines share a position.
  const byFrom = useMemo(() => {
    const map = new Map<number, RepertoireNode[]>()
    for (const n of q.data?.nodes ?? []) map.set(n.from, [...(map.get(n.from) ?? []), n])
    for (const list of map.values()) list.sort((a, b) => b.games - a.games || a.san.localeCompare(b.san))
    return map
  }, [q.data])
  // Follow the requested line. A filter change can leave it without games: stop at the deepest
  // position that has some.
  const trail = useMemo(() => {
    const out: RepertoireNode[] = []
    if (!q.data) return out
    let at = q.data.root
    for (const san of requested) {
      const n = byFrom.get(at)?.find(x => x.san === san)
      if (!n) break
      out.push(n); at = n.to
    }
    return out
  }, [q.data, byFrom, requested])
  const line = useMemo(() => (q.data ? requested.slice(0, trail.length) : requested), [q.data, requested, trail])
  const current = trail.at(-1)
  // Incoming edges can share games, so use the backend's distinct-position totals.
  const reached = useMemo(() => {
    return current && q.data?.positions.find(p => p.id === current.to)
  }, [current, q.data])
  const children = (q.data && byFrom.get(current ? current.to : q.data.root)) ?? []
  // The opening is named by the deepest position along this line that the dataset names.
  let opening: { name: string; eco: string | null } | null = null
  for (let k = trail.length - 1; k >= 0 && !opening; k--) {
    const n = trail[k]
    if (n.opening) opening = { name: n.opening, eco: n.eco }
  }
  const { fen, last } = useMemo(() => position(line), [line])
  const yourTurn = (line.length % 2 === 0) === (color === 'white')
  const tcLabel = TIME_CLASSES.find(([v]) => v === timeClass)?.[1].toLowerCase() ?? ''

  const trimmed = line.length < requested.length
    ? line.length >= plies
      ? `This view covers the first ${plies / 2} moves.${plies < 30 ? ' Choose a deeper tree to continue.' : ''}`
      : `No ${timeClass === 'all' ? '' : tcLabel + ' '}games continued with ${line.length % 2 === 0 ? `${line.length / 2 + 1}. ` : `${(line.length + 1) / 2}… `}${requested[line.length]} here, so the line stops at the deepest position that has games.`
    : null

  /** Playing a move on the board follows the tree when games went that way. */
  const onBoardMove = (orig: string, dest: string) => {
    let san: string | null = null
    try { san = new Chess(fen).move({ from: orig, to: dest, promotion: 'q' }).san } catch { /* not legal */ }
    const child = san && children.find(n => n.san === san)
    if (child) { go([...line, child.san]); return }
    setNonce(n => n + 1)
    if (san) setNotice(`No game continued with ${san} here.`)
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.metaKey || e.ctrlKey) return
      if ((e.key === 'ArrowLeft' || e.key === 'Backspace') && line.length) { e.preventDefault(); go(line.slice(0, -1)) }
      else if (e.key === 'Home') { e.preventDefault(); go([]) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  if (loading) return <Loading />
  if (!account) {
    return <div className="appear"><PageHeader title="Openings" />
      <Empty title="Connect a Chess.com account first" action={<Link to="/accounts"><Button variant="primary">Connect account</Button></Link>}>
        The repertoire is built from your own reviewed games.
      </Empty></div>
  }
  const d = q.data

  return (
    <div className="appear">
      <PageHeader title="Openings"
        description="Your reviewed games as a move tree: how each line went for you, where you leave theory, and which of your moves to fix."
        actions={<>
          <Select label="Move orders" value={merge ? '1' : '0'} onChange={v => update({ merge: v === '1' ? '1' : null })}>
            <option value="0">As played</option>
            <option value="1">Merge transpositions</option>
          </Select>
          <Select label="Tree depth" value={String(plies)} onChange={v => update({ plies: v === '16' ? null : v })}>
            <option value="16">First 8 moves</option>
            <option value="30">First 15 moves</option>
          </Select>
          <Select label="Time control" value={timeClass} onChange={v => update({ tc: v === 'all' ? null : v })}>
            {TIME_CLASSES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
          </Select>
          <Tabs label="Colour" value={color} onChange={v => { if (v !== color) update({ color: v === 'white' ? null : v, line: null, focus: null }) }}
            options={[{ value: 'white', label: 'As White' }, { value: 'black', label: 'As Black' }]} />
        </>} />

      {q.error ? <ErrorNote error={q.error} /> : !d ? <Loading label="Building your repertoire…" /> : d.games === 0 ? (
        <Card>{timeClass === 'all'
          ? <Empty title={`No reviewed games as ${color === 'white' ? 'White' : 'Black'} yet`}>Games appear here once they are reviewed.</Empty>
          : <Empty title={`No reviewed ${tcLabel} games as ${color === 'white' ? 'White' : 'Black'}`}
              action={<Button onClick={() => update({ tc: null })}>Show all time controls</Button>}>Try another time control or colour.</Empty>}</Card>
      ) : (
        <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px] xl:grid-cols-[minmax(0,1fr)_420px]">
          <div className="mx-auto w-full min-w-[280px] max-w-[620px] lg:max-w-[min(620px,calc(100vh-19rem))]">
            <Board fen={fen} orientation={color} lastMove={last} interactive nonce={nonce} onMove={onBoardMove} />
            <div className="mt-3 flex items-start gap-2">
              <Button size="icon" variant="ghost" aria-label="Back one move" disabled={!line.length} onClick={() => go(line.slice(0, -1))}><ChevronLeft className="h-4 w-4" /></Button>
              <Button size="icon" variant="ghost" aria-label="Back to the start" disabled={!line.length} onClick={() => go([])}><RotateCcw className="h-3.5 w-3.5" /></Button>
              <div className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5 pt-1.5 font-mono text-[13px]" aria-label="Current line">
                {line.length === 0 ? <span className="text-fg-3">Starting position</span> : line.map((san, i) => (
                  <button key={i} type="button" onClick={() => go(line.slice(0, i + 1))}
                    className={clsx('rounded px-0.5 hover:text-fg', i === line.length - 1 ? 'font-semibold text-fg' : 'text-fg-2')}>
                    {i % 2 === 0 ? `${i / 2 + 1}. ` : ''}{san}
                  </button>
                ))}
              </div>
            </div>
            {(notice ?? trimmed) && <p className="mt-2 text-[13px] text-fg-2" role="status">{notice ?? trimmed}</p>}
          </div>

          <div className="flex min-w-0 flex-col gap-4">
            <Card padded={false}
              title={opening ? <>{opening.name}{opening.eco && <span className="ml-2 font-normal text-fg-3">{opening.eco}</span>}</> : line.length ? 'Unnamed position' : 'Starting position'}
              subtitle={reached
                ? `${reached.games} of your games reached this position${reached.orders > 1 ? ` by ${reached.orders} move orders` : ''} · you scored ${pct(((reached.wins + reached.draws / 2) / reached.games) * 100)}`
                : `${d.games} reviewed games as ${color === 'white' ? 'White' : 'Black'} · you scored ${pct(((d.wins + d.draws / 2) / d.games) * 100)}`}>
              <p className="border-b border-line px-4 py-2.5 text-[13px] text-fg-2">
                {yourTurn ? 'Your moves from here.' : 'What your opponents played here.'} Pick one, or play it on the board.
              </p>
              {children.length ? (<>
                <div className="grid grid-cols-[4rem_2.75rem_minmax(0,1fr)_3rem] gap-x-3 px-4 pt-2 text-[12px] text-fg-3" aria-hidden>
                  <span>Move</span><span className="text-right">Games</span><span>Won · drawn · lost</span><span className="text-right">Score</span>
                </div>
                <div className="divide-y divide-line">
                  {children.map(n => <MoveRow key={`${n.from}:${n.san}`} node={n} opening={opening?.name ?? null} merged={merge} focus={focus === n.san} onClick={() => go([...line, n.san])} />)}
                </div>
              </>) : <div className="space-y-2 px-4 py-4 text-[13.5px] text-fg-2">
                <p>{line.length >= d.plies ? `The tree covers the first ${d.plies / 2} moves.` : 'None of your games continued from here.'}</p>
                {line.length >= d.plies && plies < 30 && <Button size="sm" onClick={() => update({ plies: '30' })}>Explore to move 15</Button>}
              </div>}
              {current && current.recent_games.length > 0 && (
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-line px-4 py-2.5 text-[13px]">
                  <span className="text-fg-2">See it in your games:</span>
                  {current.recent_games.map((id, i) => (
                    <Link key={id} to={`/games/${id}?ply=${current.recent_plies?.[i] ?? line.length}`} className="text-fg-2 underline-offset-2 hover:text-fg hover:underline">
                      {i === 0 ? 'Latest game' : `Game ${i + 1}`}
                    </Link>
                  ))}
                </div>
              )}
            </Card>

            {d.problems.length > 0 && (
              <Card padded={false} title="Moves to fix" subtitle="Your moves that went wrong in most games where you played them, or after which you usually lost.">
                <div className="divide-y divide-line">
                  {d.problems.map(p => {
                    const moves = p.path.split(' ')
                    return (
                      <div key={p.path} className="flex items-start gap-3 px-4 py-2.5 transition-colors hover:bg-hover/60">
                      <button type="button" onClick={() => { go(moves.slice(0, -1), p.san); window.scrollTo({ top: 0, behavior: 'smooth' }) }}
                        className="block min-w-0 flex-1 text-left">
                        <span className="block truncate font-mono text-[13px]">
                          <span className="text-fg-2">{numbered(moves.slice(0, -1))} </span>
                          <span className="font-semibold">{numbered(moves).split(' ').slice(-(moves.length % 2 === 1 ? 2 : 1)).join(' ')}</span>
                        </span>
                        <span className="mt-0.5 block text-[12.5px] text-fg-2">
                          {(() => { const s = errorSummary(p); return s ? s[0].toUpperCase() + s.slice(1) + ' games' : `${p.games} games` })()} · scored {pct(p.score * 100)}
                        </span>
                        {p.opening && <span className="mt-0.5 block truncate text-[12px] text-fg-3">{p.opening}</span>}
                      </button>
                      {p.recent_games[0] && <Link to={`/games/${p.recent_games[0]}?ply=${p.ply}`} className="shrink-0 pt-0.5 text-[12.5px] text-fg-2 hover:text-fg">Open game</Link>}
                      </div>
                    )
                  })}
                </div>
              </Card>
            )}
            {d.total_games > d.games && <p className="text-[12.5px] text-fg-3">{d.total_games - d.games} of {d.total_games} games are not reviewed yet and are not in the tree.</p>}
          </div>
        </div>
      )}
    </div>
  )
}
