import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { DrawShape } from 'chessground/draw'
import type { Key } from 'chessground/types'
import { Chess, type Square } from 'chess.js'
import clsx from 'clsx'
import {
  ArrowLeft, ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, Cpu, ExternalLink, FlipVertical2, Lightbulb, RefreshCw, Sparkles, X,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router-dom'
import Board from '../components/Board'
import ClassIcon, { ClassLabel } from '../components/ClassIcon'
import EvalBar from '../components/EvalBar'
import EvalGraph from '../components/EvalGraph'
import MoveList from '../components/MoveList'
import { Button, ErrorNote, Loading, Badge, Progress, Tabs, Spinner } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { CLASS_META, CLASS_ORDER, classBadgeSvg, PHASE_LABEL } from '../lib/classification'
import { fmtClock, fmtDate, fmtTimeControl, formatEval } from '../lib/format'
import type { Classification, GameDetail, MoveRow } from '../lib/types'

interface VarStep { fen: string; san: string; from: Key; to: Key }
interface Variation { baseFen: string; basePly: number; steps: VarStep[]; idx: number; source: 'user' | 'engine' }
type Promotion = 'q' | 'r' | 'b' | 'n'

function buildSteps(fen: string, sans: string[]): VarStep[] {
  const chess = new Chess(fen)
  const out: VarStep[] = []
  for (const san of sans) {
    try {
      const m = chess.move(san)
      out.push({ fen: chess.fen(), san: m.san, from: m.from as Key, to: m.to as Key })
    } catch { break }
  }
  return out
}

function uciSquares(uci?: string | null): [Key, Key] | null {
  if (!uci || uci.length < 4) return null
  return [uci.slice(0, 2) as Key, uci.slice(2, 4) as Key]
}

function PlayerBar({ name, rating, accuracy, clock, active, isUser }: { name: string | null; rating: number | null; accuracy?: number | null; clock?: number | null; active: boolean; isUser: boolean }) {
  return (
    <div className="flex items-center justify-between gap-3 py-1.5">
      <div className="flex min-w-0 items-center gap-2">
        <span className={clsx('truncate font-semibold', isUser && 'text-accent')}>{name ?? '?'}</span>
        {rating && <span className="text-sm text-fg-2">({rating})</span>}
        {accuracy !== null && accuracy !== undefined && <Badge className="tabular">{accuracy.toFixed(1)}%</Badge>}
      </div>
      {clock !== null && clock !== undefined && (
        <span className={clsx('rounded-md px-2 py-0.5 font-mono text-sm tnum', active ? 'bg-fg text-bg' : 'bg-hover text-fg-2')}>{fmtClock(clock)}</span>
      )}
    </div>
  )
}

function SummaryPanel({ data }: { data: GameDetail }) {
  const a = data.analysis!
  const g = data.game
  const rows = CLASS_ORDER.filter(c => (a.counts.white[c] ?? 0) + (a.counts.black[c] ?? 0) > 0 || ['brilliant', 'great', 'best', 'mistake', 'blunder'].includes(c))
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-3">
        {(['white', 'black'] as const).map(side => (
          <div key={side} className={clsx('rounded-xl border px-3 py-3 text-center', g.user_color === side ? 'border-accent/50 bg-accent/5' : 'border-line bg-subtle')}>
            <div className="truncate text-xs text-fg-2">{g[side].username}</div>
            <div className="mt-1 text-3xl font-bold tnum">{a.accuracy[side]?.toFixed(1) ?? '—'}</div>
            <div className="text-[11px] uppercase tracking-wide text-fg-3">Accuracy</div>
            <div className="mt-1 text-xs text-fg-2 tnum">ACPL {a.acpl[side] ?? '—'}</div>
            {g[side].chesscom_accuracy !== null && (
              <div className="mt-0.5 text-[11px] text-fg-3" title="Accuracy reported by Chess.com's own Game Review (different algorithm)">Chess.com: {g[side].chesscom_accuracy?.toFixed(1)}</div>
            )}
          </div>
        ))}
      </div>
      <table className="w-full text-sm">
        <tbody>
          {rows.map(c => (
            <tr key={c} className="border-b border-line/50 last:border-0">
              <td className="w-10 py-1.5 text-center font-semibold tnum" style={{ color: (a.counts.white[c] ?? 0) ? CLASS_META[c].color : undefined }}>{a.counts.white[c] ?? 0}</td>
              <td className="py-1.5"><span className="flex items-center justify-center gap-2"><ClassIcon c={c} size={16} /><span className="w-20 text-fg-2">{CLASS_META[c].label}</span></span></td>
              <td className="w-10 py-1.5 text-center font-semibold tnum" style={{ color: (a.counts.black[c] ?? 0) ? CLASS_META[c].color : undefined }}>{a.counts.black[c] ?? 0}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export default function GameReview() {
  const { id } = useParams()
  const [sp] = useSearchParams()
  return <GameReviewContent key={`${id}:${sp.get('ply') ?? ''}`} />
}

function GameReviewContent() {
  const { id } = useParams()
  const gameId = Number(id)
  const [sp] = useSearchParams()
  const { account } = useAccount()
  const qc = useQueryClient()
  const [selectedPly, setPly] = useState<number>(() => {
    const value = Number(sp.get('ply') ?? 0)
    return Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0
  })
  const [flip, setFlip] = useState(false)
  const [variation, setVariation] = useState<Variation | null>(null)
  const [showArrows, setShowArrows] = useState(true)
  const [hint, setHint] = useState(false)
  const [tab, setTab] = useState<'review' | 'moves'>('review')
  const [depth, setDepth] = useState<number>(20)
  const [promotion, setPromotion] = useState<{ orig: string; dest: string } | null>(null)
  const [enginePaused, setEnginePaused] = useState(false)
  const [engineFen, setEngineFen] = useState('')

  const q = useQuery({
    queryKey: ['game', gameId, account?.id],
    queryFn: () => api.game(gameId, account?.id),
    refetchInterval: query => (query.state.data?.game.job && query.state.data.game.job.status !== 'failed' ? 2000 : false),
  })
  const data = q.data
  const canExplore = data?.game.rules === 'chess'
  const moves = useMemo(() => data?.moves ?? [], [data])
  const n = moves.length
  const ply = Math.min(selectedPly, n)
  const analysis = data?.analysis ?? null

  const analyze = useMutation({
    mutationFn: (opts: { depth?: number; force?: boolean }) => api.analyzeGame(gameId, opts.depth, opts.force),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['game', gameId] }),
  })

  const mainFen = useCallback((p: number) => {
    if (!data) return ''
    if (p === 0) return moves[0]?.fen_before ?? data.game.initial_fen ?? ''
    return p < n ? moves[p].fen_before : data.game.final_fen ?? ''
  }, [data, moves, n])

  const go = useCallback((p: number) => { setPromotion(null); setVariation(null); setPly(Math.max(0, Math.min(n, p))) }, [n])
  const step = useCallback((d: number) => {
    if (variation) {
      const idx = variation.idx + d
      if (idx < -1) return
      if (idx >= variation.steps.length) return
      setVariation({ ...variation, idx })
      return
    }
    go(ply + d)
  }, [variation, ply, go])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (promotion) { if (e.key === 'Escape') setPromotion(null); return }
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey ||
        (e.target as HTMLElement)?.closest('input, textarea, select, [contenteditable="true"], [role="dialog"]')) return
      if (e.key === 'ArrowLeft') { e.preventDefault(); step(-1) }
      else if (e.key === 'ArrowRight') { e.preventDefault(); step(1) }
      else if (e.key === 'ArrowUp' || e.key === 'Home') { e.preventDefault(); go(0) }
      else if (e.key === 'ArrowDown' || e.key === 'End') { e.preventDefault(); go(n) }
      else if (e.key === 'f') setFlip(f => !f)
      else if (e.key === 'Escape') setVariation(null)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [step, go, n, promotion])

  const orientation: 'white' | 'black' = useMemo(() => {
    const base = data?.game.user_color ?? 'white'
    return flip ? (base === 'white' ? 'black' : 'white') : base
  }, [data, flip])

  const current: MoveRow | null = ply > 0 ? moves[ply - 1] : null
  const inVar = variation !== null
  const fen = inVar ? (variation.idx >= 0 ? variation.steps[variation.idx].fen : variation.baseFen) : mainFen(ply)
  const lastMove: [Key, Key] | null = inVar
    ? (variation.idx >= 0 ? [variation.steps[variation.idx].from, variation.steps[variation.idx].to] : null)
    : uciSquares(current?.uci)

  const liveEnabled = canExplore && !!fen && !enginePaused && (inVar || hint || !analysis)
  useEffect(() => {
    const timer = setTimeout(() => setEngineFen(fen), 150)
    return () => clearTimeout(timer)
  }, [fen])
  const live = useQuery({
    queryKey: ['engine', fen, depth],
    queryFn: ({ signal }) => api.engine(fen, depth, 3, signal),
    enabled: liveEnabled && engineFen === fen,
    staleTime: Infinity,
  })
  useEffect(() => {
    if (!liveEnabled) void qc.cancelQueries({ queryKey: ['engine', fen, depth], exact: true })
  }, [qc, fen, depth, liveEnabled])

  const shapes: DrawShape[] = useMemo(() => {
    const s: DrawShape[] = []
    if (!inVar && current?.classification && current.uci) {
      const sq = uciSquares(current.uci)
      if (sq) s.push({ orig: sq[1], customSvg: { html: classBadgeSvg(current.classification) } })
      if (showArrows && current.best_uci && current.best_uci !== current.uci &&
        ['inaccuracy', 'mistake', 'miss', 'blunder'].includes(current.classification)) {
        const b = uciSquares(current.best_uci)
        if (b) s.push({ orig: b[0], dest: b[1], brush: 'green' })
      }
    }
    if (hint || inVar) {
      const top = inVar || !analysis ? live.data?.lines?.[0]?.uci?.[0] : (ply < n ? moves[ply].best_uci : null)
      const b = uciSquares(top)
      if (b) s.push({ orig: b[0], dest: b[1], brush: 'blue' })
    }
    return s
  }, [inVar, current, showArrows, hint, live.data, analysis, ply, n, moves])

  const applyBoardMove = (orig: string, dest: string, piece?: Promotion) => {
    const chess = new Chess(fen)
    let mv
    try { mv = chess.move({ from: orig, to: dest, promotion: piece }) } catch { return }
    if (!mv) return
    const uci = `${mv.from}${mv.to}${mv.promotion ?? ''}`
    if (!inVar && ply < n && moves[ply].uci === uci) { setPly(ply + 1); return }
    const stepNew: VarStep = { fen: chess.fen(), san: mv.san, from: mv.from as Key, to: mv.to as Key }
    if (inVar) {
      const steps = [...variation.steps.slice(0, variation.idx + 1), stepNew]
      setVariation({ ...variation, steps, idx: steps.length - 1, source: 'user' })
    } else {
      setVariation({ baseFen: fen, basePly: ply, steps: [stepNew], idx: 0, source: 'user' })
    }
  }

  const onBoardMove = (orig: string, dest: string) => {
    const chess = new Chess(fen)
    if (chess.get(orig as Square)?.type === 'p' && (dest[1] === '8' || dest[1] === '1')) {
      setPromotion({ orig, dest })
    } else applyBoardMove(orig, dest)
  }

  const playLine = (baseFen: string, basePly: number, sans: string[], idx: number) => {
    if (!canExplore) return
    const steps = buildSteps(baseFen, sans)
    if (steps.length) setVariation({ baseFen, basePly, steps, idx: Math.min(idx, steps.length - 1), source: 'engine' })
  }

  if (q.isLoading) return <Loading label="Loading game…" />
  if (q.error) return <ErrorNote error={q.error} />
  if (!data) return null
  const g = data.game
  const topSide = orientation === 'white' ? 'black' : 'white'
  const bottomSide = orientation
  const clockFor = (side: 'white' | 'black') => {
    const c = side === 'white' ? 'w' : 'b'
    for (let i = Math.min(ply, n) - 1; i >= 0; i--) if (moves[i].color === c) return moves[i].clock ?? null
    return null
  }
  const evalNow = inVar || !analysis ? live.data?.lines?.[0]?.cp ?? (analysis ? analysis.evals[ply] : null) : analysis.evals[ply]
  const job = g.job

  return (
    <div className="appear">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <Link to="/games" className="rounded-lg p-2 text-fg-2 hover:bg-subtle hover:text-fg" aria-label="Back to games"><ArrowLeft className="h-5 w-5" /></Link>
          <div className="min-w-0">
            <h1 className="truncate text-xl font-bold">{g.white.username} vs {g.black.username} <span className="ml-1 text-base font-medium text-fg-2">{g.result}</span></h1>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-fg-2">
              <span>{fmtDate(g.end_time, true)}</span>
              <span>{fmtTimeControl(g.time_control)} {g.time_class}</span>
              {g.termination && <span className="truncate">{g.termination}</span>}
              {(analysis?.opening.name || g.opening.name) && <span className="text-fg/80">{analysis?.opening.eco ?? g.opening.eco} · {analysis?.opening.name ?? g.opening.name}</span>}
            </div>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {g.url && <a href={g.url} target="_blank" rel="noreferrer"><Button size="sm" variant="ghost"><ExternalLink className="h-4 w-4" />Chess.com</Button></a>}
          <a href={`https://lichess.org/analysis/${g.rules === 'chess960' ? 'chess960/' : ''}${fen.replace(/ /g, '_')}`} target="_blank" rel="noreferrer"><Button size="sm" variant="ghost"><ExternalLink className="h-4 w-4" />Lichess</Button></a>
        </div>
      </div>

      {!analysis && (
        <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-xl border border-accent-ink/40 bg-accent-ink/10 px-4 py-3">
          <div className="flex items-center gap-3 text-sm">
            {job && job.status !== 'failed' ? <Spinner /> : <Cpu className="h-5 w-5 text-accent-ink" />}
            <span>
              {job?.status === 'running' ? `Analysing with Stockfish… ${job.progress ?? 0}/${job.total ?? '?'} positions`
                : job?.status === 'queued' ? 'Queued for analysis.'
                  : job?.status === 'failed' ? `Analysis failed: ${job.error}`
                    : g.pgn_error ? `This game can't be analysed: ${g.pgn_error}` : "This game hasn't been analysed yet."}
            </span>
          </div>
          {job?.status === 'running' && job.total ? <Progress value={job.progress ?? 0} max={job.total} className="max-w-xs" /> : null}
          {(!job || job.status === 'failed') && !g.pgn_error && (
            <Button variant="primary" size="sm" loading={analyze.isPending} onClick={() => analyze.mutate({})}><Sparkles className="h-4 w-4" />Analyse now</Button>
          )}
        </div>
      )}
      {!canExplore && <p className="mb-4 rounded-lg border border-line bg-surface p-3 text-sm text-fg-2">Chess960 games support recorded-move review. Board exploration and live engine hints are available for standard chess.</p>}

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_420px]">
        {/* board column */}
        <div className="mx-auto w-full max-w-[min(720px,calc(100vh-150px))] min-w-[280px]">
          <PlayerBar name={g[topSide].username} rating={g[topSide].rating} accuracy={analysis?.accuracy[topSide]} clock={clockFor(topSide)}
            active={!inVar && ply < n && moves[ply]?.color === topSide[0]} isUser={g.user_color === topSide} />
          <div className="flex gap-2">
            <EvalBar cp={evalNow} orientation={orientation} />
            <div className="min-w-0 flex-1">
              <Board fen={fen} orientation={orientation} lastMove={lastMove} shapes={shapes} interactive={canExplore && !promotion} onMove={onBoardMove} />
            </div>
          </div>
          {promotion && <div className="mt-3 flex flex-wrap items-center gap-2 rounded-lg border border-line bg-surface p-3" role="group" aria-label="Promote pawn">
            <span className="mr-1 text-sm">Promote to</span>
            {([{ piece: 'q', label: 'Queen' }, { piece: 'r', label: 'Rook' }, { piece: 'b', label: 'Bishop' }, { piece: 'n', label: 'Knight' }] as const).map(p => <Button key={p.piece} onClick={() => { applyBoardMove(promotion.orig, promotion.dest, p.piece); setPromotion(null) }}>{p.label}</Button>)}
            <Button variant="ghost" onClick={() => setPromotion(null)}>Cancel promotion</Button>
          </div>}
          <PlayerBar name={g[bottomSide].username} rating={g[bottomSide].rating} accuracy={analysis?.accuracy[bottomSide]} clock={clockFor(bottomSide)}
            active={!inVar && ply < n && moves[ply]?.color === bottomSide[0]} isUser={g.user_color === bottomSide} />

          <div className="mt-2 flex items-center justify-between gap-2">
            <div className="flex items-center gap-1">
              <Button size="sm" variant="ghost" onClick={() => go(0)} aria-label="First move"><ChevronFirst className="h-5 w-5" /></Button>
              <Button size="sm" variant="secondary" onClick={() => step(-1)} aria-label="Previous move"><ChevronLeft className="h-5 w-5" /></Button>
              <Button size="sm" variant="secondary" onClick={() => step(1)} aria-label="Next move"><ChevronRight className="h-5 w-5" /></Button>
              <Button size="sm" variant="ghost" onClick={() => go(n)} aria-label="Last move"><ChevronLast className="h-5 w-5" /></Button>
            </div>
            <div className="flex items-center gap-1">
              <Button size="sm" disabled={!canExplore} variant={hint ? 'primary' : 'ghost'} onClick={() => { setHint(h => !h); setEnginePaused(false) }} title="Show the engine's best move in this position"><Lightbulb className="h-4 w-4" />Hint</Button>
              <Button size="sm" variant={showArrows ? 'secondary' : 'ghost'} onClick={() => setShowArrows(s => !s)} title="Show better-move arrows on errors">Arrows</Button>
              <Button size="sm" variant="ghost" onClick={() => setFlip(f => !f)} aria-label="Flip board" title="Flip board (f)"><FlipVertical2 className="h-4 w-4" /></Button>
            </div>
          </div>
          {analysis && (
            <div className="mt-3 card p-2">
              <EvalGraph wins={analysis.win_white} evals={analysis.evals} moves={moves} current={inVar ? variation.basePly : ply} onSelect={go} />
            </div>
          )}
        </div>

        {/* side panel */}
        <div className="flex min-w-0 flex-col gap-4">
          {inVar && (
            <div className="card border-accent-ink/50 p-4">
              <div className="mb-2 flex items-center justify-between">
                <div className="text-sm font-semibold text-accent-ink">{variation.source === 'engine' ? 'Engine line' : 'Your variation'} <span className="font-normal text-fg-2">from move {Math.ceil((variation.basePly + 1) / 2)}</span></div>
                <Button size="sm" variant="ghost" onClick={() => setVariation(null)}><X className="h-4 w-4" />Back to game</Button>
              </div>
              <div className="flex flex-wrap gap-1 font-mono text-sm">
                {variation.steps.map((s, i) => (
                  <button key={i} onClick={() => setVariation({ ...variation, idx: i })}
                    className={clsx('rounded px-1.5 py-0.5', i === variation.idx ? 'bg-accent-ink/30 text-fg' : 'text-fg-2 hover:bg-subtle')}>{s.san}</button>
                ))}
              </div>
            </div>
          )}

          <div className="card">
            <div className="flex border-b border-line">
              {(['review', 'moves'] as const).map(t => (
                <button key={t} onClick={() => setTab(t)} className={clsx('flex-1 px-4 py-3 text-sm font-semibold capitalize', tab === t ? 'border-b-2 border-accent text-fg' : 'text-fg-2 hover:text-fg')}>{t}</button>
              ))}
            </div>
            <div className="p-4">
              {tab === 'review' ? (
                <div className="space-y-4">
                  <MoveInsight current={current} ply={ply} analysis={!!analysis} allowExploration={canExplore} onPlayLine={(sans, idx) => current && playLine(current.fen_before, ply - 1, sans, idx)} />
                  {analysis ? <SummaryPanel data={data} /> : <MoveList moves={moves} current={inVar ? -1 : ply} onSelect={go} />}
                  {analysis && analysis.key_moments.length > 0 && (
                    <div>
                      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-3">Key moments</div>
                      <div className="flex flex-wrap gap-1.5">
                        {analysis.key_moments.map(p => {
                          const m = moves[p - 1]
                          if (!m?.classification) return null
                          return (
                            <button key={p} onClick={() => go(p)} className={clsx('flex items-center gap-1.5 rounded-lg border px-2 py-1 text-xs', p === ply ? 'border-accent bg-hover' : 'border-line bg-subtle hover:bg-hover')}>
                              <ClassIcon c={m.classification} size={14} />
                              <span className="font-mono">{Math.ceil(p / 2)}{p % 2 ? '.' : '…'} {m.san}</span>
                            </button>
                          )
                        })}
                      </div>
                    </div>
                  )}
                </div>
              ) : (
                <MoveList moves={moves} current={inVar ? -1 : ply} onSelect={go} />
              )}
            </div>
          </div>

          {canExplore && (inVar || hint || !analysis) && (
            <div className="card p-4">
              <div className="mb-2 flex items-center justify-between">
                <div className="flex items-center gap-2 text-sm font-semibold"><Cpu className="h-4 w-4 text-accent-ink" />Live engine</div>
                <Button size="sm" onClick={() => setEnginePaused(p => !p)}>{enginePaused ? 'Start engine' : 'Stop engine'}</Button>
                <Tabs value={String(depth)} onChange={v => setDepth(Number(v))}
                  options={[{ value: '16', label: 'd16' }, { value: '20', label: 'd20' }, { value: '24', label: 'd24' }]} />
              </div>
              {enginePaused ? <div className="py-3 text-sm text-fg-2">Engine paused.</div> : live.isFetching && !live.data ? <div className="flex items-center gap-2 py-3 text-sm text-fg-2"><Spinner className="h-4 w-4" />Thinking…</div>
                : live.error ? <ErrorNote error={live.error} />
                  : live.data?.lines.length ? (
                    <div className="space-y-1.5">
                      {live.data.lines.map((l, i) => (
                        <EngineLineRow key={i} evalText={l.eval} sans={l.san} depth={l.depth} onClick={k => playLine(fen, inVar ? variation.basePly : ply, l.san, k)} />
                      ))}
                    </div>
                  ) : <div className="text-sm text-fg-2">Game over.</div>}
            </div>
          )}

          {analysis && (
            <div className="flex flex-wrap items-center justify-between gap-2 px-1 text-xs text-fg-3">
              <span>{analysis.engine} · depth {analysis.depth} · MultiPV {analysis.multipv} · {fmtDate(analysis.analyzed_at, true)}</span>
              <span className="flex items-center gap-2">
                {analysis.outdated && <span className="text-warn">older algorithm</span>}
                <Button size="sm" variant="ghost" loading={analyze.isPending || (!!job && job.status !== 'failed')}
                  onClick={() => analyze.mutate({ depth: Math.min(30, analysis.depth + 4), force: true })}>
                  <RefreshCw className="h-3.5 w-3.5" />Deeper (d{Math.min(30, analysis.depth + 4)})
                </Button>
              </span>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function EngineLineRow({ evalText, sans, depth, onClick, disabled }: { evalText: string; sans: string[]; depth?: number; onClick: (idx: number) => void; disabled?: boolean }) {
  const positive = !evalText.startsWith('-')
  return (
    <div className="flex items-start gap-2 rounded-lg bg-subtle px-2 py-1.5">
      <span className={clsx('w-14 shrink-0 rounded px-1 py-0.5 text-center font-mono text-xs font-bold', positive ? 'bg-[#e9eaee] text-[#222]' : 'bg-[#3a3f47] text-white')}>{evalText}</span>
      <div className="flex min-w-0 flex-wrap gap-x-1 font-mono text-[13px] leading-6">
        {sans.slice(0, 10).map((s, i) => (
          <button key={i} disabled={disabled} onClick={() => onClick(i)} className="rounded px-0.5 text-fg/90 enabled:hover:bg-hover enabled:hover:text-fg">{s}</button>
        ))}
      </div>
      {depth ? <span className="ml-auto shrink-0 text-[10px] text-fg-3">d{depth}</span> : null}
    </div>
  )
}

function MoveInsight({ current, ply, analysis, onPlayLine, allowExploration }: { current: MoveRow | null; ply: number; analysis: boolean; onPlayLine: (sans: string[], idx: number) => void; allowExploration: boolean }) {
  if (!analysis) return null
  if (!current) {
    return (
      <div className="rounded-xl bg-subtle p-4 text-sm text-fg-2">
        Step through the game with <kbd className="rounded bg-hover px-1">←</kbd> <kbd className="rounded bg-hover px-1">→</kbd>, click the graph, or drag pieces to explore your own ideas.
      </div>
    )
  }
  const c = current.classification as Classification | undefined
  const moveNo = `${Math.ceil(ply / 2)}${ply % 2 ? '.' : '…'}`
  return (
    <div className="rounded-xl bg-subtle p-4" style={c ? { boxShadow: `inset 3px 0 0 ${CLASS_META[c].color}` } : undefined}>
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          {c && <ClassLabel c={c} />}
          <span className="font-mono text-sm text-fg-2">{moveNo} {current.san}</span>
        </div>
        <span className="font-mono text-sm font-semibold">{formatEval(current.eval_after)}</span>
      </div>
      {current.explanation && <p className="mt-2 text-sm leading-relaxed text-fg/90">{current.explanation}</p>}
      <div className="mt-2 flex flex-wrap gap-2 text-xs text-fg-3">
        {current.phase && <span>{PHASE_LABEL[current.phase]}</span>}
        {current.accuracy !== undefined && <span>Move accuracy {current.accuracy.toFixed(0)}%</span>}
        {current.ep_loss !== undefined && current.ep_loss > 0 && <span>−{(current.ep_loss * 100).toFixed(1)}% win chance</span>}
        {current.time_spent !== null && current.time_spent !== undefined && <span>{current.time_spent.toFixed(1)}s spent</span>}
      </div>
      {current.lines && current.lines.length > 0 && (
        <div className="mt-3 space-y-1.5">
          <div className="text-[11px] font-semibold uppercase tracking-wide text-fg-3">Engine lines before {current.san}</div>
          {current.lines.map((l, i) => (
            <EngineLineRow key={i} disabled={!allowExploration} evalText={l.eval} sans={l.san} depth={l.depth} onClick={k => onPlayLine(l.san, k)} />
          ))}
        </div>
      )}
    </div>
  )
}
