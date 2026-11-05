import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Chess, type Square } from 'chess.js'
import clsx from 'clsx'
import { ArrowRight, Check, ChevronLeft, ChevronRight, Eye, Lightbulb, RotateCcw, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import Board from '../components/Board'
import ClassIcon from '../components/ClassIcon'
import { Button, Card, Empty, ErrorNote, Loading, PageHeader, Select, Tabs } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { CLASS_META, PHASE_LABEL } from '../lib/classification'
import { cap, DAY, fmtDate, fmtDuration, nowTs } from '../lib/format'
import type { InteractiveResult, PracticeItem, PracticeKind, PracticePhase, PracticeResult, PracticeStats } from '../lib/types'

type Kind = 'all' | PracticeKind
type Phase = 'all' | PracticePhase
type Range = 'all' | 'today' | '7' | '30'
type Promotion = 'q' | 'r' | 'b' | 'n'
type Status = 'solving' | 'checking' | 'solved' | 'revealed'

const KINDS: { value: Kind; label: string }[] = [
  { value: 'all', label: 'All' }, { value: 'blunder', label: 'Blunders' },
  { value: 'mistake', label: 'Mistakes' }, { value: 'miss', label: 'Misses' },
]
const RANGES: [Range, string][] = [['all', 'All games, due first'], ['today', 'Today’s games'], ['7', 'Last 7 days'], ['30', 'Last 30 days']]
const PHASES: [Phase, string][] = [['all', 'All phases'], ['opening', 'Opening'], ['middlegame', 'Middlegame'], ['endgame', 'Endgame']]
/** With "Play out the line", a puzzle asks for at most this many of your moves. */
const MAX_OWN_MOVES = 3
/** A different continuation is accepted when it keeps this much winning chance (percentage points). */
const ALT_MARGIN = 5
/** Depth and lines for that check: plenty to compare two moves, and fast enough to feel instant. */
const CHECK_DEPTH = 14
const CHECK_LINES = 4

function stored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try { const v = localStorage.getItem(key); return allowed.includes(v as T) ? v as T : fallback } catch { return fallback }
}
function store(key: string, value: string) { try { localStorage.setItem(key, value) } catch { /* ignore */ } }

function since(range: Range): number | undefined {
  if (range === 'all') return undefined
  if (range === 'today') { const d = new Date(); d.setHours(0, 0, 0, 0); return Math.floor(d.getTime() / 1000) }
  return nowTs() - Number(range) * DAY
}

function squares(uci: string | null | undefined): [Key, Key] | null {
  return uci && uci.length >= 4 ? [uci.slice(0, 2) as Key, uci.slice(2, 4) as Key] : null
}

/** Apply SAN moves from a position; returns each position and move. */
function walk(fen: string, sans: string[]) {
  const chess = new Chess(fen)
  const out: { fen: string; san: string; from: Key; to: Key }[] = []
  for (const san of sans) {
    try { const m = chess.move(san); out.push({ fen: chess.fen(), san: m.san, from: m.from as Key, to: m.to as Key }) } catch { break }
  }
  return out
}

function returnsIn(due: number) {
  const s = Math.max(0, due - nowTs())
  if (s < 3600) return 'later in this session'
  if (s < 43200) return `in ${fmtDuration(s)}`
  const days = Math.max(1, Math.round(s / 86400))
  return days === 1 ? 'tomorrow' : `in ${days} days`
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" aria-checked={checked} onClick={() => onChange(!checked)}
      className="inline-flex h-10 items-center gap-2 rounded-md border border-line bg-surface px-2.5 text-[13px] font-medium text-fg-2 transition-colors hover:text-fg sm:h-8">
      <span className={clsx('relative h-4 w-7 rounded-full transition-colors', checked ? 'bg-accent' : 'bg-line-strong')}>
        <span className={clsx('absolute top-0.5 h-3 w-3 rounded-full bg-white shadow-sm transition-[left]', checked ? 'left-[14px]' : 'left-0.5')} />
      </span>
      {label}
    </button>
  )
}

export default function Practice() {
  const { account } = useAccount()
  // A different account starts a different session, including pending answers and engine checks.
  return <PracticeSession key={account?.id ?? 'none'} />
}

function PracticeSession() {
  const { account, loading } = useAccount()
  const qc = useQueryClient()
  const [kind, setKind] = useState<Kind>(() => stored('ci.practiceKind', ['all', 'blunder', 'mistake', 'miss'], 'all'))
  const [phase, setPhase] = useState<Phase>(() => stored('ci.practicePhase', ['all', 'opening', 'middlegame', 'endgame'], 'all'))
  const [range, setRange] = useState<Range>(() => stored('ci.practiceRange', ['all', 'today', '7', '30'], 'all'))
  const [fullLine, setFullLine] = useState(() => stored('ci.practiceLine', ['on', 'off'], 'on') === 'on')
  const [index, setIndex] = useState(0)
  const [solution, setSolution] = useState<PracticeResult | null>(null)
  const [played, setPlayed] = useState<string[]>([])
  const [status, setStatus] = useState<Status>('solving')
  const [tries, setTries] = useState(0)
  const [wrong, setWrong] = useState<string | null>(null)
  const [hint, setHint] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  const [step, setStep] = useState<number | null>(null)
  const [score, setScore] = useState({ right: 0, done: 0 })
  const [stats, setStats] = useState<PracticeStats | null>(null)
  const [promotion, setPromotion] = useState<{ orig: string; dest: string } | null>(null)
  const [checkError, setCheckError] = useState<unknown>(null)
  const reply = useRef<number | undefined>(undefined)
  const generation = useRef(0)

  const queue = useQuery({
    queryKey: ['practice', account?.id, kind, range, phase],
    queryFn: () => api.practice(account!.id, kind === 'all' ? undefined : kind, range === 'all' ? 10 : 30, since(range), phase === 'all' ? undefined : phase),
    enabled: !!account,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  })
  const items = queue.data?.items ?? []
  const item: PracticeItem | undefined = items[index]
  const shownStats = stats ?? queue.data?.stats
  const answer = useMutation({
    mutationFn: (body: { uci: string } | { reveal: true }) => api.answerPractice(account!.id, item!.game_id, item!.ply, body),
  })
  const { reset: resetAnswer } = answer

  /** One engine check at a time, started as soon as it is your move in a line. */
  const check = useRef<{ fen: string; result: Promise<InteractiveResult>; abort: AbortController } | null>(null)
  const analyse = useCallback((fen: string) => {
    if (check.current?.fen === fen) return check.current.result
    check.current?.abort.abort()
    const abort = new AbortController()
    const result = api.engine(fen, CHECK_DEPTH, CHECK_LINES, abort.signal)
    result.catch(() => { /* awaited later, or cancelled */ })
    check.current = { fen, result, abort }
    return result
  }, [])
  const cancelCheck = useCallback(() => { check.current?.abort.abort(); check.current = null }, [])

  const resetPuzzle = useCallback(() => {
    generation.current++
    window.clearTimeout(reply.current)
    cancelCheck()
    resetAnswer()
    setSolution(null); setPlayed([]); setStatus('solving'); setTries(0); setWrong(null); setHint(false)
    setNote(null); setStep(null); setPromotion(null); setCheckError(null)
  }, [cancelCheck, resetAnswer])
  const restart = () => {
    resetPuzzle(); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
    void qc.invalidateQueries({ queryKey: ['practice', account?.id] })
  }
  const choose = <T extends string>(key: string, set: (v: T) => void) => (v: T) => {
    store(key, v); set(v); resetPuzzle(); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
  }
  const next = useCallback(() => { resetPuzzle(); setIndex(i => i + 1) }, [resetPuzzle])
  useEffect(() => () => { generation.current++; window.clearTimeout(reply.current); cancelCheck() }, [cancelCheck])

  // Background review completion marks queues stale centrally. An empty batch can become ready;
  // active and completed batches stay in place until the user chooses the next round.
  const { data: queueData, isStale, isFetching, isError, refetch } = queue
  useEffect(() => {
    if (!queueData || queueData.items.length > 0 || !isStale || isFetching || isError) return
    const current = generation.current
    void refetch().then(result => {
      if (generation.current !== current) return
      if (!result.error) { resetPuzzle(); setIndex(0); setStats(null) }
    })
  }, [queueData, isStale, isFetching, isError, refetch, resetPuzzle])

  const updateStats = (result: PracticeResult, current: number) => {
    if (kind === 'all' && phase === 'all' && range === 'all') { setStats(result.stats); return }
    // The answer response contains account totals. Fetch only filtered stats without replacing
    // the active batch (recent-game queues are shuffled by the server).
    void api.practice(account!.id, kind === 'all' ? undefined : kind, 1, since(range), phase === 'all' ? undefined : phase)
      .then(q => { if (generation.current === current) setStats(q.stats) })
      .catch(() => { /* Keep the last known stats; the answer and schedule were already saved. */ })
  }

  const line = useMemo(() => solution?.line_san ?? [], [solution])
  const ownTarget = fullLine ? Math.min(MAX_OWN_MOVES, Math.ceil(line.length / 2)) : 1
  const done = status === 'solved' || status === 'revealed'
  const firstTry = !!solution && solution.correct

  const finish = (how: Status, result: PracticeResult | null = solution) => {
    setStatus(how)
    if (result) setScore(s => ({ right: s.right + (result.correct ? 1 : 0), done: s.done + 1 }))
  }

  /** Accept the expected move, then play the opponent's reply from the line. */
  const advance = (san: string, result: PracticeResult) => {
    const moves = [...played, san]
    setPlayed(moves); setWrong(null); setHint(false)
    const own = Math.ceil(moves.length / 2)
    const target = fullLine ? Math.min(MAX_OWN_MOVES, Math.ceil(result.line_san.length / 2)) : 1
    if (own >= target || moves.length >= result.line_san.length) { finish('solved', result); return }
    const opponent = result.line_san[moves.length]
    reply.current = window.setTimeout(() => {
      const after = [...moves, opponent]
      setPlayed(after)
      if (after.length >= result.line_san.length) finish('solved', result)
    }, 450)
  }

  const reject = (san: string) => { setWrong(san); setTries(t => t + 1); setNonce(n => n + 1) }
  const currentFen = () => (item && played.length ? walk(item.fen, played).at(-1)!.fen : item?.fen ?? '')

  const onMove = async (orig: string, dest: string, piece?: Promotion) => {
    if (!item || done || answer.isPending || status === 'checking') return
    const current = generation.current
    setCheckError(null)
    const fen = currentFen()
    let mv
    try { mv = new Chess(fen).move({ from: orig, to: dest, promotion: piece }) } catch { setNonce(n => n + 1); return }
    const uci = `${mv.from}${mv.to}${mv.promotion ?? ''}`
    if (!solution) {
      // The first try is recorded for scheduling and returns the solution.
      const r = await answer.mutateAsync({ uci }).catch(() => null)
      if (generation.current !== current) return
      if (!r) { setNonce(n => n + 1); return }
      updateStats(r, current)
      setSolution(r)
      if (r.correct) advance(mv.san, r)
      else reject(mv.san)
      return
    }
    let want
    try { want = new Chess(fen).move(line[played.length]) } catch { want = null }
    if (want && want.from === mv.from && want.to === mv.to && (want.promotion ?? '') === (mv.promotion ?? '')) {
      advance(mv.san, solution); return
    }
    if (played.length === 0) { reject(mv.san); return }
    // Later in the line more than one move can be good, so ask the engine.
    setStatus('checking')
    try {
      const r = await analyse(fen)
      if (generation.current !== current) return
      const white = fen.split(' ')[1] === 'w'
      const side = (w: number) => (white ? w : 100 - w)
      const best = r.lines[0]
      const mine = r.lines.find(l => l.uci[0] === uci)
      if (best && mine && side(best.win_white) - side(mine.win_white) <= ALT_MARGIN) {
        setPlayed(p => [...p, mv.san])
        setNote(`${mv.san} works too: the engine rates it about as well as ${line[played.length]}.`)
        finish('solved')
      } else { setStatus('solving'); reject(mv.san) }
    } catch (error) {
      if (generation.current !== current) return
      cancelCheck()
      setStatus('solving'); setCheckError(error); setNonce(n => n + 1)
    }
  }

  const onBoardMove = (orig: string, dest: string) => {
    if (!item) return
    const p = new Chess(currentFen()).get(orig as Square)
    if (p?.type === 'p' && (dest[1] === '8' || dest[1] === '1')) setPromotion({ orig, dest })
    else void onMove(orig, dest)
  }

  const reveal = async () => {
    const current = ++generation.current
    window.clearTimeout(reply.current)
    cancelCheck()
    setCheckError(null)
    let r = solution
    if (!r) {
      r = await answer.mutateAsync({ reveal: true }).catch(() => null)
      if (!r || generation.current !== current) return
      updateStats(r, current)
      setSolution(r)
    }
    setStep(0)
    finish('revealed', r)
  }

  const positions = useMemo(() => (item && solution ? walk(item.fen, line) : []), [item, solution, line])
  const playedPositions = useMemo(() => (item ? walk(item.fen, played) : []), [item, played])
  const at = step ?? playedPositions.length

  // Once the opponent has replied, check the position in the background while you think,
  // so a different but good move can be accepted without waiting.
  const thinkingFen = status === 'solving' && solution && played.length > 0 && played.length % 2 === 0
    ? playedPositions.at(-1)?.fen : undefined
  useEffect(() => { if (thinkingFen) void analyse(thinkingFen) }, [thinkingFen, analyse])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.metaKey || e.ctrlKey) return
      if (!done) return
      if (e.key === 'Enter' || e.key === 'n') { e.preventDefault(); next() }
      else if (e.key === 'ArrowRight') { e.preventDefault(); setStep(Math.min(positions.length, at + 1)) }
      else if (e.key === 'ArrowLeft') { e.preventDefault(); setStep(Math.max(0, at - 1)) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [done, next, positions.length, at])

  if (loading) return <Loading />
  if (!account) {
    return <div className="appear"><PageHeader title="Practice" />
      <Empty title="Connect a Chess.com account first" action={<Link to="/accounts"><Button variant="primary">Connect account</Button></Link>}>
        Practice uses positions from your own reviewed games.
      </Empty></div>
  }

  const header = (
    <PageHeader title="Practice"
      description="Positions from your games where a clearly better move was there. Find it; the ones you miss come back."
      actions={<>
        <Select label="Game phase" value={phase} onChange={v => choose<Phase>('ci.practicePhase', setPhase)(v as Phase)}>
          {PHASES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </Select>
        <Select label="Which games" value={range} onChange={v => choose<Range>('ci.practiceRange', setRange)(v as Range)}>
          {RANGES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </Select>
        <Tabs label="Mistake type" value={kind} onChange={choose<Kind>('ci.practiceKind', setKind)} options={KINDS} />
        <Toggle label="Play out the line" checked={fullLine} onChange={v => { store('ci.practiceLine', v ? 'on' : 'off'); setFullLine(v) }} />
      </>} />
  )
  const progress = shownStats ? (
    <Card padded={false}>
      <dl className="grid grid-cols-4 divide-x divide-line">
        {([['Due', shownStats.due], ['New', shownStats.new], ['Learning', shownStats.learning], ['Mastered', shownStats.mastered]] as const).map(([label, value]) => (
          <div key={label} className="px-3 py-2.5">
            <dt className="text-[12px] text-fg-2">{label}</dt>
            <dd className="tnum mt-0.5 text-[17px] font-semibold">{value}</dd>
          </div>
        ))}
      </dl>
      <div className="border-t border-line px-3 py-2 text-[12.5px] text-fg-2">
        {score.done > 0 ? `This session: ${score.right} of ${score.done} on the first try` : `${shownStats.total} positions${range === 'all' ? '' : ' from these games'} with one clear answer`}
        {range === 'all' && shownStats.next_due_at && !shownStats.due ? ` · next review ${returnsIn(shownStats.next_due_at)}` : ''}
      </div>
    </Card>
  ) : null

  // The board shows the line being solved, or a step through the engine line afterwards.
  const viewing = done && step !== null ? positions.slice(0, step) : playedPositions
  const shownMove = viewing.at(-1)
  const fen = shownMove?.fen ?? item?.fen
  const lastMove = shownMove ? [shownMove.from, shownMove.to] as [Key, Key] : squares(item?.last_uci)
  const yourTurn = played.length % 2 === 0
  const shapes: DrawShape[] = []
  if (hint && !done && solution && line[played.length] && fen) {
    const to = walk(fen, [line[played.length]])[0]
    if (to) shapes.push({ orig: to.from, brush: 'yellow' })
  }
  if (status === 'revealed' && at === 0 && solution) {
    const best = squares(solution.best_uci)
    if (best) shapes.push({ orig: best[0], dest: best[1], brush: 'green' })
  }
  const ownDone = Math.ceil(played.length / 2)

  return (
    <div className="appear">
      {header}
      {queue.error ? <ErrorNote error={queue.error} />
        : queue.isLoading ? <Loading label="Finding positions…" />
        : !item ? (
          <div className="space-y-4">{progress}<Card>
            {shownStats?.total === 0
              ? <Empty title={kind !== 'all' || phase !== 'all' ? 'No positions match these filters' : range === 'all' ? 'No practice positions yet' : 'No positions from these games'}>
                  {kind !== 'all' || phase !== 'all' ? 'Try another game phase or mistake type, or a longer range of games.' : range === 'all' ? 'Positions appear once your games are reviewed. Only moves with one clearly best answer are used, so every puzzle has a fair solution.' : 'Pick a longer range, or wait until your latest games are reviewed.'}
                </Empty>
              : <Empty title={score.done ? `Done: ${score.right} of ${score.done} on the first try` : 'Nothing due right now'}
                  action={<Button variant="primary" onClick={restart}><RotateCcw className="h-3.5 w-3.5" />{range !== 'all' || shownStats?.due || shownStats?.new ? 'Keep going' : 'Check again'}</Button>}>
                  {range !== 'all' ? 'Go round again with the same games in a new order.'
                    : shownStats?.due || shownStats?.new ? 'More positions are ready.'
                    : shownStats?.next_due_at ? `The next review is due ${returnsIn(shownStats.next_due_at)}.` : 'You have answered every position.'}
                </Empty>}
          </Card></div>
        ) : (
          <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_360px] xl:grid-cols-[minmax(0,1fr)_400px]">
            <div className="mx-auto w-full min-w-[280px] max-w-[620px] lg:max-w-[min(640px,calc(100vh-18.5rem))]">
              <Board fen={fen!} orientation={item.color} lastMove={lastMove} shapes={shapes} nonce={nonce}
                interactive={!done && yourTurn && status === 'solving' && !answer.isPending && !promotion} onMove={onBoardMove} />
              {promotion && (
                <div className="mt-3 flex flex-wrap items-center gap-2 rounded-lg border border-line bg-surface p-3" role="group" aria-label="Promote pawn">
                  <span className="mr-1 text-[13px] text-fg-2">Promote to</span>
                  {([['q', 'Queen'], ['r', 'Rook'], ['b', 'Bishop'], ['n', 'Knight']] as const).map(([p, label]) =>
                    <Button key={p} onClick={() => { const { orig, dest } = promotion; setPromotion(null); void onMove(orig, dest, p) }}>{label}</Button>)}
                  <Button variant="ghost" onClick={() => { setPromotion(null); setNonce(n => n + 1) }}>Cancel</Button>
                </div>
              )}
            </div>

            <div className="flex min-w-0 flex-col gap-4">
              <Card>
                <div className="flex items-center gap-2.5">
                  <ClassIcon c={item.classification} size={22} />
                  <div className="min-w-0">
                    <div className="font-semibold">{cap(item.color)} to move</div>
                    <div className="text-[13px] text-fg-2">
                      Move {item.move_number}{item.last_san ? ` after ${item.last_san}` : ''}{item.phase ? ` · ${PHASE_LABEL[item.phase] ?? item.phase}` : ''}
                    </div>
                  </div>
                  <span className="tnum ml-auto text-[13px] text-fg-3">{index + 1} / {items.length}</span>
                </div>
                <p className="mt-3 text-[14px] text-fg-2">
                  {item.status === 'new' ? 'You played a' : 'You once played a'} <span style={{ color: CLASS_META[item.classification].color }} className="font-medium">{CLASS_META[item.classification].label.toLowerCase()}</span> here
                  {item.opponent ? <> against {item.opponent}{item.opponent_rating ? ` (${item.opponent_rating})` : ''}</> : null}
                  {item.end_time ? `, ${fmtDate(item.end_time)}` : ''}.
                </p>
                {!done && (
                  <div className="mt-3 text-[13.5px]" aria-live="polite">
                    {status === 'checking' ? <span className="text-fg-2">Checking that move with the engine…</span>
                      : wrong ? <span className="flex items-center gap-1.5 text-bad"><X className="h-4 w-4" />{wrong} isn’t it. Try again.</span>
                      : played.length > 0 && yourTurn ? <span className="flex items-center gap-1.5 text-good"><Check className="h-4 w-4" />Good. Find the next move ({ownDone + 1} of {ownTarget}).</span>
                      : played.length > 0 ? <span className="text-fg-2">Your opponent replies…</span>
                      : <span className="text-fg-2">Find the best move{fullLine ? ', then keep going' : ''}.</span>}
                  </div>
                )}
                {!done && (
                  <div className="mt-4 flex flex-wrap items-center gap-2">
                    {solution && !hint && <Button variant="ghost" onClick={() => setHint(true)}><Lightbulb className="h-3.5 w-3.5" />Hint</Button>}
                    <Button variant="ghost" onClick={() => void reveal()} loading={answer.isPending && !solution}><Eye className="h-3.5 w-3.5" />Show answer</Button>
                    <Button variant="ghost" onClick={next}>Skip</Button>
                  </div>
                )}
                {answer.error && <div className="mt-3"><ErrorNote error={answer.error} /></div>}
                {!!checkError && <div className="mt-3"><ErrorNote error={checkError} /><p className="mt-2 text-[13px] text-fg-2">The move wasn’t counted as wrong. Try it again, or use a hint to follow the stored line.</p></div>}
              </Card>

              {done && solution && (
                <Card>
                  <div className={clsx('flex items-center gap-2 font-semibold', status === 'solved' && firstTry ? 'text-good' : status === 'solved' ? 'text-fg' : 'text-bad')}>
                    {status === 'solved' ? <Check className="h-4 w-4" /> : <X className="h-4 w-4" />}
                    {status === 'solved' && firstTry ? `Solved${ownTarget > 1 ? ' the line' : ''} on the first try`
                      : status === 'solved' ? `Solved after ${tries} wrong ${tries === 1 ? 'try' : 'tries'}`
                      : `The move was ${solution.best_san}`}
                  </div>
                  <p className="mt-2 text-[13.5px] text-fg-2">
                    {note ? `${note} ` : ''}In the game you played {solution.game_san}. {firstTry ? 'It comes back' : 'Only the first try counts, so it comes back'} {returnsIn(solution.due_at)}.
                  </p>
                  {positions.length > 0 && (
                    <div className="mt-4">
                      <div className="mb-2 flex items-center justify-between">
                        <span className="text-[13px] font-medium">Engine line</span>
                        <div className="flex gap-1">
                          <Button size="icon" variant="ghost" aria-label="Previous move in line" disabled={at === 0} onClick={() => setStep(Math.max(0, at - 1))}><ChevronLeft className="h-4 w-4" /></Button>
                          <Button size="icon" variant="ghost" aria-label="Next move in line" disabled={at >= positions.length} onClick={() => setStep(Math.min(positions.length, at + 1))}><ChevronRight className="h-4 w-4" /></Button>
                        </div>
                      </div>
                      <div className="flex flex-wrap gap-x-1.5 gap-y-1 font-mono text-[13px]">
                        {positions.map((m, i) => (
                          <button key={i} type="button" onClick={() => setStep(i + 1)}
                            className={clsx('rounded px-1 transition-colors', at === i + 1 ? 'bg-hover text-fg' : 'text-fg-2 hover:text-fg')}>
                            {m.san}
                          </button>
                        ))}
                      </div>
                    </div>
                  )}
                  <div className="mt-5 flex flex-wrap items-center gap-2">
                    <Button variant="primary" onClick={next}>Next<ArrowRight className="h-3.5 w-3.5" /></Button>
                    <Link to={`/games/${item.game_id}?ply=${item.ply - 1}`} className="text-[13px] text-fg-2 hover:text-fg">Open in game review</Link>
                  </div>
                </Card>
              )}
              {progress}
            </div>
          </div>
        )}
    </div>
  )
}
