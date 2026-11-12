import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Chess, type Move, type Square } from 'chess.js'
import clsx from 'clsx'
import { ArrowRight, Check, ChevronLeft, ChevronRight, Copy, ExternalLink, Eye, Lightbulb, RotateCcw, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import Board from '../components/Board'
import ClassIcon from '../components/ClassIcon'
import PromotionPicker, { type Promotion } from '../components/PromotionPicker'
import { Button, Card, Empty, ErrorNote, Input, Loading, PageHeader, Select, Tabs } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { CLASS_META, PHASE_LABEL } from '../lib/classification'
import { cap, DAY, dueIn, fmtDate, nowTs } from '../lib/format'
import type { InteractiveResult, PracticeItem, PracticeKind, PracticePhase, PracticeResult, PracticeStats, PracticeTheme } from '../lib/types'

type Kind = 'all' | PracticeKind
type Phase = 'all' | PracticePhase
type Theme = 'all' | PracticeTheme
type Range = 'all' | 'today' | '7' | '30'
type Status = 'solving' | 'checking' | 'solved' | 'revealed'

const KINDS: { value: Kind; label: string }[] = [
  { value: 'all', label: 'All' }, { value: 'blunder', label: 'Blunders' },
  { value: 'mistake', label: 'Mistakes' }, { value: 'miss', label: 'Misses' },
]
const RANGES: [Range, string][] = [['all', 'All games, due first'], ['today', 'Today’s games'], ['7', 'Last 7 days'], ['30', 'Last 30 days']]
const PHASES: [Phase, string][] = [['all', 'All phases'], ['opening', 'Opening'], ['middlegame', 'Middlegame'], ['endgame', 'Endgame']]
const THEMES: [PracticeTheme, string][] = [
  ['mate-in-1', 'Mate in 1'], ['mate-in-2', 'Mate in 2'], ['mate-in-3-plus', 'Mate in 3+'],
  ['back-rank-mate', 'Back-rank mate'], ['fork', 'Fork'], ['knight-fork', 'Knight fork'],
  ['pin', 'Pin'], ['skewer', 'Skewer'], ['discovered-attack', 'Discovered attack'],
  ['hanging-piece', 'Hanging piece'], ['trapped-piece', 'Trapped piece'], ['promotion', 'Promotion'], ['defensive', 'Defensive'],
]
const THEME_LABEL = Object.fromEntries(THEMES) as Record<PracticeTheme, string>
/** With "Play out the line", a puzzle asks for at most this many of your moves. */
const MAX_OWN_MOVES = 3
/** A different continuation is accepted when it keeps this much winning chance (percentage points). */
const ALT_MARGIN = 5
/** Depth and lines for that check: plenty to compare two moves, and fast enough to feel instant. */
const CHECK_DEPTH = 14
const CHECK_LINES = 4
/** An engine check that has not answered by then is abandoned; the move is not counted. */
const CHECK_TIMEOUT_MS = 10_000
/** After this long without an answer, say why it is slow and what to do. */
const SLOW_MS = 5_000

class CheckTimeout extends Error {
  constructor() { super('The engine didn’t answer in time. Reviews running in the background can slow it down.') }
}

/** Resolve with the check, or reject once it has taken longer than the user should wait. */
function withinTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: number | undefined
  return Promise.race([
    promise,
    new Promise<never>((_, reject) => { timer = window.setTimeout(() => reject(new CheckTimeout()), ms) }),
  ]).finally(() => window.clearTimeout(timer))
}

/** True once `active` has stayed true for `ms`. */
function useSlow(active: boolean, ms: number) {
  const [slow, setSlow] = useState(false)
  useEffect(() => {
    if (!active) return
    const timer = window.setTimeout(() => setSlow(true), ms)
    return () => { window.clearTimeout(timer); setSlow(false) }
  }, [active, ms])
  return slow
}

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

function returnsIn(due: number) { return dueIn(due, nowTs()) }

/** The puzzle as PGN: its position, the engine line and what was played in the game. */
function puzzlePgn(item: PracticeItem, solution: PracticeResult, line: string[], username: string) {
  const chess = new Chess(item.fen)
  chess.setComment(`In the game: ${solution.game_san}. Best: ${solution.best_san}.`)
  for (const san of line) { try { chess.move(san) } catch { break } }
  const date = item.end_time ? new Date(item.end_time * 1000) : null
  const [white, black] = item.color === 'white' ? [username, item.opponent ?? '?'] : [item.opponent ?? '?', username]
  chess.setHeader('Event', 'Plyback practice')
  chess.setHeader('Date', date ? `${date.getFullYear()}.${String(date.getMonth() + 1).padStart(2, '0')}.${String(date.getDate()).padStart(2, '0')}` : '????.??.??')
  chess.setHeader('White', white)
  chess.setHeader('Black', black)
  return chess.pgn()
}

/** Copy text, also where the clipboard API is unavailable (a plain-http address on the network). */
async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text); return } catch { /* fall back below */ }
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  const ok = document.execCommand('copy')
  area.remove()
  if (!ok) throw new Error('Copy failed')
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
  const [theme, setTheme] = useState<Theme>(() => stored('ci.practiceTheme', ['all', ...THEMES.map(([value]) => value)], 'all'))
  const [range, setRange] = useState<Range>(() => stored('ci.practiceRange', ['all', 'today', '7', '30'], 'all'))
  const [fullLine, setFullLine] = useState(() => stored('ci.practiceLine', ['on', 'off'], 'on') === 'on')
  /** The "Play out the line" setting this puzzle uses. A change mid-puzzle applies from the next one. */
  const [lineFor, setLineFor] = useState(fullLine)
  const fullLineRef = useRef(fullLine)
  const [index, setIndex] = useState(0)
  const [solution, setSolution] = useState<PracticeResult | null>(null)
  const [played, setPlayed] = useState<string[]>([])
  const [status, setStatus] = useState<Status>('solving')
  const [tries, setTries] = useState(0)
  const [wrong, setWrong] = useState<string | null>(null)
  const [hint, setHint] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  /** After an accepted alternative: the moves so far followed by the engine's line from there. */
  const [liveLine, setLiveLine] = useState<string[] | null>(null)
  const [nonce, setNonce] = useState(0)
  const [step, setStep] = useState<number | null>(null)
  const [score, setScore] = useState({ right: 0, done: 0 })
  const [stats, setStats] = useState<PracticeStats | null>(null)
  const [promotion, setPromotion] = useState<{ orig: string; dest: string } | null>(null)
  /** A move being checked stays on the board until it is accepted or put back. */
  const [trying, setTrying] = useState<{ fen: string; from: Key; to: Key } | null>(null)
  const [checkError, setCheckError] = useState<unknown>(null)
  const [typed, setTyped] = useState('')
  const [typedError, setTypedError] = useState<string | null>(null)
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const reply = useRef<number | undefined>(undefined)
  const generation = useRef(0)

  const queue = useQuery({
    queryKey: ['practice', account?.id, kind, range, phase, theme],
    queryFn: () => api.practice(account!.id, kind === 'all' ? undefined : kind, range === 'all' ? 10 : 30, since(range), phase === 'all' ? undefined : phase, theme === 'all' ? undefined : theme),
    enabled: !!account,
    staleTime: Infinity,
    // Finished reviews mark queues stale. Refetch only on purpose (a new round, an empty batch),
    // never on remount or reconnect, so the batch can't be reordered under a puzzle in progress.
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    refetchOnReconnect: false,
    // Leaving the page or switching a filter away drops the batch, so coming back fetches the
    // positions still due instead of showing ones answered a minute ago as new.
    gcTime: 0,
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
    setNote(null); setLiveLine(null); setStep(null); setPromotion(null); setTrying(null); setCheckError(null)
    setLineFor(fullLineRef.current); setTyped(''); setTypedError(null); setCopied(null)
  }, [cancelCheck, resetAnswer])
  const restart = () => {
    resetPuzzle(); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
    void qc.resetQueries({ queryKey: ['practice', account?.id] })
  }
  const choose = <T extends string>(key: string, set: (v: T) => void) => (v: T) => {
    store(key, v); set(v); resetPuzzle(); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
  }
  const next = useCallback(() => { resetPuzzle(); setIndex(i => i + 1) }, [resetPuzzle])
  useEffect(() => () => { generation.current++; window.clearTimeout(reply.current); cancelCheck() }, [cancelCheck])
  // If the position under the board changes for any other reason, start it fresh.
  const itemKey = item ? `${item.game_id}:${item.ply}` : ''
  const shownKey = useRef(itemKey)
  useEffect(() => {
    if (shownKey.current !== itemKey) { shownKey.current = itemKey; resetPuzzle() }
  }, [itemKey, resetPuzzle])

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
    if (kind === 'all' && phase === 'all' && range === 'all' && theme === 'all') { setStats(result.stats); return }
    // The answer response contains account totals. Fetch only filtered stats without replacing
    // the active batch (recent-game queues are shuffled by the server).
    void api.practice(account!.id, kind === 'all' ? undefined : kind, 1, since(range), phase === 'all' ? undefined : phase, theme === 'all' ? undefined : theme)
      .then(q => { if (generation.current === current) setStats(q.stats) })
      .catch(() => { /* Keep the last known stats; the answer and schedule were already saved. */ })
  }

  const line = useMemo(() => liveLine ?? solution?.line_san ?? [], [liveLine, solution])
  const ownTarget = lineFor ? Math.min(MAX_OWN_MOVES, Math.ceil(line.length / 2)) : 1
  const done = status === 'solved' || status === 'revealed'
  const firstTry = !!solution && solution.correct
  const clean = firstTry && tries === 0

  const finish = (how: Status, result: PracticeResult | null = solution) => {
    setStatus(how)
    if (result) setScore(s => ({ right: s.right + (result.correct ? 1 : 0), done: s.done + 1 }))
  }

  /** Accept the expected move, then play the opponent's reply from the line. */
  const advance = (san: string, result: PracticeResult, path: string[] = line) => {
    const moves = [...played, san]
    setPlayed(moves); setWrong(null); setHint(false)
    const own = Math.ceil(moves.length / 2)
    const target = lineFor ? Math.min(MAX_OWN_MOVES, Math.ceil(path.length / 2)) : 1
    if (own >= target || moves.length >= path.length) { finish('solved', result); return }
    const opponent = path[moves.length]
    reply.current = window.setTimeout(() => {
      const after = [...moves, opponent]
      setPlayed(after)
      if (after.length >= path.length) finish('solved', result)
    }, 450)
  }

  const reject = (san: string) => { setWrong(san); setTries(t => t + 1); setNonce(n => n + 1) }
  const currentFen = () => (item && played.length ? walk(item.fen, played).at(-1)!.fen : item?.fen ?? '')

  const onMove = async (orig: string, dest: string, piece?: Promotion) => {
    // Every path out of here changes the position or puts the board back, so the board
    // never keeps a half-made move with no legal moves left (#29).
    if (!item || done || answer.isPending || status === 'checking') { setNonce(n => n + 1); return }
    const current = generation.current
    setCheckError(null)
    const fen = currentFen()
    let mv
    try { mv = new Chess(fen).move({ from: orig, to: dest, promotion: piece }) } catch { setNonce(n => n + 1); return }
    const uci = `${mv.from}${mv.to}${mv.promotion ?? ''}`
    setTrying({ fen: mv.after, from: mv.from as Key, to: mv.to as Key })
    try { await judge(mv, uci, fen, current) } finally { if (generation.current === current) setTrying(null) }
  }

  const judge = async (mv: Move, uci: string, fen: string, current: number) => {
    if (!solution) {
      // The first try is recorded for scheduling and returns the solution.
      const r = await answer.mutateAsync({ uci }).catch(() => null)
      if (generation.current !== current) return
      if (!r) { setNonce(n => n + 1); return }
      updateStats(r, current)
      setSolution(r)
      if (r.correct) advance(mv.san, r, r.line_san)
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
      const r = await withinTimeout(analyse(fen), CHECK_TIMEOUT_MS)
      if (generation.current !== current) return
      const white = fen.split(' ')[1] === 'w'
      const side = (w: number) => (white ? w : 100 - w)
      const best = r.lines[0]
      const mine = r.lines.find(l => l.uci[0] === uci)
      if (best && mine && side(best.win_white) - side(mine.win_white) <= ALT_MARGIN) {
        // The stored line no longer applies, so continue with the engine's line after this move.
        const continued = mine.san[0] === mv.san ? [...played, ...mine.san] : [...played, mv.san]
        setLiveLine(continued); setStatus('solving')
        setNote(`${mv.san} works too: the engine rates it about as well as ${line[played.length]}.${continued.length > played.length + 1 && lineFor ? ' The line continues from the engine’s analysis.' : ''}`)
        advance(mv.san, solution, continued)
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
    setCheckError(null); setTrying(null); setPromotion(null)
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

  const waiting = status === 'checking' || (answer.isPending && !solution)
  const slow = useSlow(waiting, SLOW_MS)
  const cancelPromotion = useCallback(() => { setPromotion(null); setNonce(n => n + 1) }, [])

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
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented) return
      // Enter on a focused link or button activates it; it is not "next puzzle".
      if (e.key === 'Enter' && e.target instanceof Element && e.target.closest('a,button,textarea,[role="dialog"]')) return
      if (promotion) { if (e.key === 'Escape') { e.preventDefault(); cancelPromotion() } return }
      if (!done) return
      if (e.key === 'Enter' || e.key === 'n') { e.preventDefault(); next() }
      else if (e.key === 'ArrowRight') { e.preventDefault(); setStep(Math.min(positions.length, at + 1)) }
      else if (e.key === 'ArrowLeft') { e.preventDefault(); setStep(Math.max(0, at - 1)) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [done, next, positions.length, at, promotion, cancelPromotion])

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
        <Select label="Tactical theme" value={theme} onChange={v => choose<Theme>('ci.practiceTheme', setTheme)(v as Theme)}>
          <option value="all">All themes</option>
          {THEMES.filter(([value]) => value === theme || (queue.data?.theme_counts?.[value] ?? 0) > 0).map(([value, label]) => <option key={value} value={value}>{label} ({queue.data?.theme_counts?.[value] ?? 0})</option>)}
        </Select>
        <Select label="Which games" value={range} onChange={v => choose<Range>('ci.practiceRange', setRange)(v as Range)}>
          {RANGES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </Select>
        <Tabs label="Mistake type" value={kind} onChange={choose<Kind>('ci.practiceKind', setKind)} options={KINDS} />
        <Toggle label="Play out the line" checked={fullLine} onChange={v => {
          store('ci.practiceLine', v ? 'on' : 'off'); setFullLine(v); fullLineRef.current = v
          // Before the first move the puzzle can still change; after that it keeps its rules.
          if (!played.length && !solution) setLineFor(v)
        }} />
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
        {score.done > 0 ? `This session: ${score.right} of ${score.done} on the first try` : `${shownStats.total} ${shownStats.total === 1 ? 'position' : 'positions'}${range === 'all' ? '' : ' from these games'} with one clear answer`}
        {range === 'all' && shownStats.next_due_at && !shownStats.due ? ` · next review ${returnsIn(shownStats.next_due_at)}` : ''}
      </div>
    </Card>
  ) : null

  // The board shows the line being solved, or a step through the engine line afterwards.
  const viewing = done && step !== null ? positions.slice(0, step) : playedPositions
  const shownMove = trying ?? viewing.at(-1)
  const fen = shownMove?.fen ?? item?.fen
  const lastMove = shownMove ? [shownMove.from, shownMove.to] as [Key, Key] : squares(item?.last_uci)
  const yourTurn = played.length % 2 === 0
  const canMove = !done && yourTurn && status === 'solving' && !answer.isPending && !promotion
  const typeMove = () => {
    const text = typed.trim()
    if (!text || !canMove) return
    const position = new Chess(currentFen())
    let m
    try { m = position.move(text) } catch {
      try { m = /^[a-h][1-8][a-h][1-8][qrbn]?$/i.test(text) ? position.move({ from: text.slice(0, 2).toLowerCase(), to: text.slice(2, 4).toLowerCase(), promotion: text[4]?.toLowerCase() }) : null } catch { m = null }
    }
    if (!m) { setTypedError(`${text} isn’t a legal move here.`); return }
    setTyped(''); setTypedError(null)
    void onMove(m.from, m.to, m.promotion as Promotion | undefined)
  }
  const shapes: DrawShape[] = []
  if (hint && !done && yourTurn && solution && line[played.length] && fen) {
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
              ? <Empty title={kind !== 'all' || phase !== 'all' || theme !== 'all' ? 'No positions match these filters' : range === 'all' ? 'No practice positions yet' : 'No positions from these games'}>
                  {kind !== 'all' || phase !== 'all' || theme !== 'all' ? 'Try another theme, game phase or mistake type, or a longer range of games.' : range === 'all' ? 'Positions appear once your games are reviewed. Only moves with one clearly best answer are used, so every puzzle has a fair solution.' : 'Pick a longer range, or wait until your latest games are reviewed.'}
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
            <div className="relative mx-auto w-full min-w-[280px] max-w-[620px] lg:max-w-[min(640px,calc(100vh-18.5rem))]">
              <Board fen={fen!} orientation={item.color} lastMove={lastMove} shapes={shapes} nonce={nonce}
                interactive={canMove} onMove={onBoardMove} />
              {promotion && <PromotionPicker onCancel={cancelPromotion}
                onPick={p => { const { orig, dest } = promotion; setPromotion(null); void onMove(orig, dest, p) }} />}
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
                      : answer.isPending && !solution ? <span className="text-fg-2">Checking your move…</span>
                      : promotion ? <span className="text-fg-2">Choose a piece to promote to.</span>
                      : wrong ? <span className="flex items-center gap-1.5 text-bad"><X className="h-4 w-4" />{wrong} isn’t it. Try again.</span>
                      : played.length > 0 && yourTurn ? <span className="flex items-center gap-1.5 text-good"><Check className="h-4 w-4" />Good. Find the next move ({ownDone + 1} of {ownTarget}).</span>
                      : played.length > 0 ? <span className="text-fg-2">Your opponent replies…</span>
                      : <span className="text-fg-2">Find the best move{lineFor ? ', then keep going' : ''}.</span>}
                  </div>
                )}
                {!done && (
                  // Moves can also be typed, so Practice works from the keyboard and with a screen reader.
                  <form className="mt-3 flex items-center gap-2" onSubmit={e => { e.preventDefault(); typeMove() }}>
                    <Input aria-label="Type your move" placeholder="Type a move, e.g. Nf3 or g1f3" value={typed} disabled={!canMove}
                      onChange={e => { setTyped(e.target.value); setTypedError(null) }} className="w-56" autoComplete="off" spellCheck={false} />
                    <Button type="submit" size="sm" disabled={!canMove || !typed.trim()}>Play</Button>
                    {typedError && <span className="text-[13px] text-bad" role="alert">{typedError}</span>}
                  </form>
                )}
                {!done && (
                  <div className="mt-4 flex flex-wrap items-center gap-2">
                    {solution && !hint && yourTurn && <Button variant="ghost" onClick={() => setHint(true)}><Lightbulb className="h-3.5 w-3.5" />Hint</Button>}
                    <Button variant="ghost" onClick={() => void reveal()} loading={answer.isPending && !solution}><Eye className="h-3.5 w-3.5" />Show answer</Button>
                    <Button variant="ghost" onClick={next}>Skip</Button>
                  </div>
                )}
                {!done && lineFor !== fullLine && <p className="mt-2 text-[13px] text-fg-2">“Play out the line” changes from the next position.</p>}
                {!done && waiting && slow && (
                  <p className="mt-2 text-[13px] text-fg-2" role="status">
                    This is taking longer than usual. Reviews running in the background can slow it down. You can wait, or skip this position.
                  </p>
                )}
                {!done && note && <p className="mt-2 text-[13px] text-fg-2">{note}</p>}
                {answer.error && <div className="mt-3"><ErrorNote error={answer.error} /></div>}
                {!!checkError && <div className="mt-3"><ErrorNote error={checkError} /><p className="mt-2 text-[13px] text-fg-2">The move wasn’t counted as wrong. Play it again to retry, or use a hint to follow the stored line.</p></div>}
              </Card>

              {done && solution && (
                <Card>
                  <div className={clsx('flex items-center gap-2 font-semibold', status === 'solved' && clean ? 'text-good' : status === 'solved' ? 'text-fg' : 'text-bad')}>
                    {status === 'solved' ? <Check className="h-4 w-4" /> : <X className="h-4 w-4" />}
                    {status === 'solved' && clean ? `Solved${ownTarget > 1 ? ' the line' : ''} on the first try`
                      : status === 'solved' && firstTry ? `First move right; solved the line after ${tries} wrong ${tries === 1 ? 'try' : 'tries'}`
                      : status === 'solved' ? `Solved after ${tries} wrong ${tries === 1 ? 'try' : 'tries'}`
                      : `The move was ${solution.best_san}`}
                  </div>
                  <p className="mt-2 text-[13.5px] text-fg-2">
                    {note ? `${note} ` : ''}In the game you played {solution.game_san}. {firstTry ? 'It comes back' : 'Only the first try counts, so it comes back'} {returnsIn(solution.due_at)}.
                  </p>
                  {(solution.themes ?? item.themes ?? []).length > 0 && (
                    <p className="mt-3 border-t border-line pt-3 text-[13px] text-fg-2" aria-label="Puzzle themes">
                      <span className="font-semibold text-fg">Themes: </span>{(solution.themes ?? item.themes).map(value => THEME_LABEL[value]).join(', ')}
                    </p>
                  )}
                  {positions.length > 0 && (
                    <div className="mt-4">
                      <div className="mb-2 flex items-center justify-between">
                        <span className="text-[13px] font-medium">{liveLine ? 'Engine line, with your alternative' : 'Engine line'}</span>
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
                    <Button variant="ghost" onClick={() => {
                      copyText(puzzlePgn(item, solution, line, account.username)).then(() => setCopied('yes'), () => setCopied('failed'))
                    }}><Copy className="h-3.5 w-3.5" />{copied === 'yes' ? 'Copied' : 'Copy PGN'}</Button>
                    <a href={`https://lichess.org/analysis/${item.fen.replace(/ /g, '_')}?color=${item.color}`} target="_blank" rel="noreferrer"
                      className="inline-flex items-center gap-1 text-[13px] text-fg-2 hover:text-fg">Analyse on Lichess<ExternalLink className="h-3.5 w-3.5" /></a>
                    <Link to={`/games/${item.game_id}?ply=${item.ply - 1}`} className="text-[13px] text-fg-2 hover:text-fg">Open in game review</Link>
                  </div>
                  {copied === 'failed' && <p className="mt-2 text-[13px] text-bad" role="alert">Couldn’t copy. Your browser blocked the clipboard.</p>}
                </Card>
              )}
              {progress}
            </div>
          </div>
        )}
    </div>
  )
}
