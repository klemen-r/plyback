import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { useMutation } from '@tanstack/react-query'
import { Chess, type Move } from 'chess.js'
import clsx from 'clsx'
import { ArrowRight, Check, ChevronLeft, ChevronRight, Copy, ExternalLink, Eye, Lightbulb, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useReducer, useRef, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import Board, { type BoardMove } from './Board'
import ClassIcon from './ClassIcon'
import { Button, Card, ErrorNote, Input } from './ui'
import { api } from '../lib/api'
import { CLASS_META, PHASE_LABEL } from '../lib/classification'
import { uciSquares as squares } from '../lib/chess'
import { cap, dueIn, fmtDate, nowTs } from '../lib/format'
import { alternativeLine, initialPuzzleState, practiceReducer, puzzlePgn, PRACTICE_THEME_LABEL as THEME_LABEL } from '../lib/practice'
import type { Account, InteractiveResult, PracticeItem, PracticeResult } from '../lib/types'
import { buildSteps as walk } from '../lib/variation'

/** With "Play out the line", ask for at most this many of your moves. */
const MAX_OWN_MOVES = 3
/** Enough depth and lines to compare two moves without a full game review. */
const CHECK_DEPTH = 14
const CHECK_LINES = 4
const CHECK_TIMEOUT_MS = 10_000
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

interface Props {
  account: Account
  item: PracticeItem
  index: number
  total: number
  fullLine: boolean
  progress: ReactNode
  onNext: () => void
  onAnswered: (result: PracticeResult, signal: AbortSignal) => void
  onFinished: (result: PracticeResult) => void
}

/** A keyed instance owns one position, including its pending answers and engine checks. */
export default function PracticePuzzle({ account, item, index, total, fullLine, progress, onNext: next, onAnswered, onFinished }: Props) {
  const [state, dispatch] = useReducer(practiceReducer, fullLine, initialPuzzleState)
  const { solution, played, status, tries, wrong, hint, note, liveLine, step, trying, error: checkError, typed, typedError, copied, lineFor } = state
  const reply = useRef<number | undefined>(undefined)
  const work = useRef(new AbortController())
  const moving = useRef(false)
  const pendingAnswer = useRef(false)
  const finished = useRef(false)
  const answer = useMutation({
    mutationFn: (body: { uci: string } | { reveal: true }) => api.answerPractice(account.id, item.game_id, item.ply, body),
  })

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
  useEffect(() => {
    const abort = new AbortController()
    work.current = abort
    return () => { work.current.abort(); window.clearTimeout(reply.current); cancelCheck() }
  }, [cancelCheck])
  useEffect(() => { dispatch({ type: 'setting', fullLine }) }, [fullLine])

  const line = useMemo(() => liveLine ?? solution?.line_san ?? [], [liveLine, solution])
  const ownTarget = lineFor ? Math.min(MAX_OWN_MOVES, Math.ceil(line.length / 2)) : 1
  const done = status === 'solved' || status === 'revealed'
  const firstTry = !!solution && solution.correct
  const clean = firstTry && tries === 0

  const finish = (how: 'solved' | 'revealed', result: PracticeResult) => {
    dispatch({ type: 'finish', status: how })
    if (!finished.current) { finished.current = true; onFinished(result) }
  }

  /** Accept the expected move, then play the opponent's reply from the line. */
  const advance = (san: string, result: PracticeResult, path: string[] = line) => {
    const moves = [...played, san]
    dispatch({ type: 'play', moves })
    const own = Math.ceil(moves.length / 2)
    const target = lineFor ? Math.min(MAX_OWN_MOVES, Math.ceil(path.length / 2)) : 1
    if (own >= target || moves.length >= path.length) { finish('solved', result); return }
    const opponent = path[moves.length]
    reply.current = window.setTimeout(() => {
      const after = [...moves, opponent]
      dispatch({ type: 'play', moves: after })
      if (after.length >= path.length) finish('solved', result)
    }, 450)
  }

  const reject = (san: string) => { dispatch({ type: 'reject', san }); return false }
  const currentFen = () => played.length ? walk(item.fen, played).at(-1)!.fen : item.fen
  const requestAnswer = async (body: { uci: string } | { reveal: true }) => {
    pendingAnswer.current = true
    try { return await answer.mutateAsync(body).catch(() => null) }
    finally { pendingAnswer.current = false }
  }

  const judge = async (mv: Move, uci: string, fen: string, signal: AbortSignal): Promise<boolean> => {
    if (!solution) {
      // Let this scheduling write finish even if the position is skipped; its stale UI effects
      // belong to this puzzle's abort signal and cannot update the next position.
      const result = await requestAnswer({ uci })
      if (signal.aborted || !result) return false
      onAnswered(result, signal)
      dispatch({ type: 'answer', result })
      if (result.correct) { advance(mv.san, result, result.line_san); return true }
      return reject(mv.san)
    }
    let want
    try { want = new Chess(fen).move(line[played.length]) } catch { want = null }
    if (want && want.from === mv.from && want.to === mv.to && (want.promotion ?? '') === (mv.promotion ?? '')) {
      advance(mv.san, solution); return true
    }
    if (played.length === 0) return reject(mv.san)
    // Later in the line more than one move can be good, so ask the engine.
    dispatch({ type: 'checking' })
    try {
      const result = await withinTimeout(analyse(fen), CHECK_TIMEOUT_MS)
      if (signal.aborted) return false
      const continued = alternativeLine(result, fen, uci, mv.san, played)
      if (!continued) return reject(mv.san)
      dispatch({ type: 'alternative', line: continued,
        note: `${mv.san} works too: the engine rates it about as well as ${line[played.length]}.${continued.length > played.length + 1 && lineFor ? ' The line continues from the engine’s analysis.' : ''}` })
      advance(mv.san, solution, continued)
      return true
    } catch (error) {
      if (signal.aborted) return false
      cancelCheck()
      dispatch({ type: 'error', error })
      return false
    }
  }

  const onMove = async ({ from, to, promotion }: BoardMove): Promise<boolean> => {
    if (done || moving.current || pendingAnswer.current || status === 'checking') return false
    const signal = work.current.signal
    if (signal.aborted) return false
    const fen = currentFen()
    let move
    try { move = new Chess(fen).move({ from, to, promotion }) } catch { return false }
    moving.current = true
    dispatch({ type: 'trying', move: { fen: move.after, from: move.from as Key, to: move.to as Key } })
    try { return await judge(move, `${move.from}${move.to}${move.promotion ?? ''}`, fen, signal) }
    finally {
      if (!signal.aborted) { moving.current = false; dispatch({ type: 'trying', move: null }) }
    }
  }

  const reveal = async () => {
    if (pendingAnswer.current || finished.current) return
    work.current.abort()
    work.current = new AbortController()
    const signal = work.current.signal
    window.clearTimeout(reply.current)
    cancelCheck()
    moving.current = false
    dispatch({ type: 'trying', move: null })
    let result = solution
    if (!result) {
      result = await requestAnswer({ reveal: true })
      if (!result || signal.aborted) return
      onAnswered(result, signal)
      dispatch({ type: 'answer', result })
    }
    finish('revealed', result)
  }

  const waiting = status === 'checking' || (answer.isPending && !solution)
  const slow = useSlow(waiting, SLOW_MS)
  const positions = useMemo(() => solution ? walk(item.fen, line) : [], [item, solution, line])
  const playedPositions = useMemo(() => walk(item.fen, played), [item, played])
  const at = step ?? playedPositions.length

  // Once the opponent has replied, check the position in the background while you think,
  // so a different but good move can be accepted without waiting.
  const thinkingFen = status === 'solving' && solution && played.length > 0 && played.length % 2 === 0
    ? playedPositions.at(-1)?.fen : undefined
  useEffect(() => { if (thinkingFen) void analyse(thinkingFen) }, [thinkingFen, analyse])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement || event.metaKey || event.ctrlKey || event.altKey || event.defaultPrevented) return
      // Enter on a focused link or button activates it; it is not "next puzzle".
      if (event.key === 'Enter' && event.target instanceof Element && event.target.closest('a,button,textarea,[role="dialog"]')) return
      if (!done) return
      if (event.key === 'Enter' || event.key === 'n') { event.preventDefault(); next() }
      else if (event.key === 'ArrowRight') { event.preventDefault(); dispatch({ type: 'seek', step: Math.min(positions.length, at + 1) }) }
      else if (event.key === 'ArrowLeft') { event.preventDefault(); dispatch({ type: 'seek', step: Math.max(0, at - 1) }) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [done, next, positions.length, at])

  // The board shows the line being solved, or a step through the engine line afterwards.
  const viewing = done && step !== null ? positions.slice(0, step) : playedPositions
  const shownMove = trying ?? viewing.at(-1)
  const fen = shownMove?.fen ?? item.fen
  const lastMove = shownMove ? [shownMove.from, shownMove.to] as [Key, Key] : squares(item.last_uci)
  const yourTurn = played.length % 2 === 0
  const canMove = !done && yourTurn && status === 'solving' && !answer.isPending
  const typeMove = () => {
    const text = typed.trim()
    if (!text || !canMove || moving.current) return
    const position = new Chess(currentFen())
    let move
    try { move = position.move(text) } catch {
      try { move = /^[a-h][1-8][a-h][1-8][qrbn]?$/i.test(text) ? position.move({ from: text.slice(0, 2).toLowerCase(), to: text.slice(2, 4).toLowerCase(), promotion: text[4]?.toLowerCase() }) : null } catch { move = null }
    }
    if (!move) { dispatch({ type: 'typedError', text: `${text} isn’t a legal move here.` }); return }
    dispatch({ type: 'type', text: '' })
    void onMove({ from: move.from, to: move.to, promotion: move.promotion as BoardMove['promotion'] })
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
    <Board fen={fen} orientation={item.color} lastMove={lastMove} shapes={shapes} interactive={canMove} onMove={onMove}>
      {(board, promoting) => <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_360px] xl:grid-cols-[minmax(0,1fr)_400px]">
            <div className="relative mx-auto w-full min-w-[280px] max-w-[620px] lg:max-w-[min(640px,calc(100vh-18.5rem))]">
              {board}

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
                  <span className="tnum ml-auto text-[13px] text-fg-3">{index + 1} / {total}</span>
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
                      : promoting ? <span className="text-fg-2">Choose a piece to promote to.</span>
                      : wrong ? <span className="flex items-center gap-1.5 text-bad"><X className="h-4 w-4" />{wrong} isn’t it. Try again.</span>
                      : played.length > 0 && yourTurn ? <span className="flex items-center gap-1.5 text-good"><Check className="h-4 w-4" />Good. Find the next move ({ownDone + 1} of {ownTarget}).</span>
                      : played.length > 0 ? <span className="text-fg-2">Your opponent replies…</span>
                      : <span className="text-fg-2">Find the best move{lineFor ? ', then keep going' : ''}.</span>}
                  </div>
                )}
                {!done && (
                  // Moves can also be typed, so Practice works from the keyboard and with a screen reader.
                  <form className="mt-3 flex items-center gap-2" onSubmit={e => { e.preventDefault(); if (!promoting) typeMove() }}>
                    <Input aria-label="Type your move" placeholder="Type a move, e.g. Nf3 or g1f3" value={typed} disabled={!canMove || promoting}
                      onChange={e => { dispatch({ type: 'type', text: e.target.value }) }} className="w-56" autoComplete="off" spellCheck={false} />
                    <Button type="submit" size="sm" disabled={!canMove || promoting || !typed.trim()}>Play</Button>
                    {typedError && <span className="text-[13px] text-bad" role="alert">{typedError}</span>}
                  </form>
                )}
                {!done && (
                  <div className="mt-4 flex flex-wrap items-center gap-2">
                    {solution && !hint && yourTurn && <Button variant="ghost" onClick={() => dispatch({ type: 'hint' })}><Lightbulb className="h-3.5 w-3.5" />Hint</Button>}
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
                    {note ? `${note} ` : ''}In the game you played {solution.game_san}. {firstTry ? 'It comes back' : 'Only the first try counts, so it comes back'} {dueIn(solution.due_at, nowTs())}.
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
                          <Button size="icon" variant="ghost" aria-label="Previous move in line" disabled={at === 0} onClick={() => dispatch({ type: 'seek', step: Math.max(0, at - 1) })}><ChevronLeft className="h-4 w-4" /></Button>
                          <Button size="icon" variant="ghost" aria-label="Next move in line" disabled={at >= positions.length} onClick={() => dispatch({ type: 'seek', step: Math.min(positions.length, at + 1) })}><ChevronRight className="h-4 w-4" /></Button>
                        </div>
                      </div>
                      <div className="flex flex-wrap gap-x-1.5 gap-y-1 font-mono text-[13px]">
                        {positions.map((m, i) => (
                          <button key={i} type="button" onClick={() => dispatch({ type: 'seek', step: i + 1 })}
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
                      copyText(puzzlePgn(item, solution, line, account.username)).then(() => dispatch({ type: 'copy', result: 'yes' }), () => dispatch({ type: 'copy', result: 'failed' }))
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
          </div>}
    </Board>
  )
}
