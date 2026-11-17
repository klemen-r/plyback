import { useQuery, useQueryClient } from '@tanstack/react-query'
import { RotateCcw } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import PracticePuzzle from '../components/PracticePuzzle'
import { Button, Card, Empty, ErrorNote, Loading, PageHeader, Select, Switch, Tabs } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { dueIn, nowTs } from '../lib/format'
import { practiceSince as since, PRACTICE_THEMES as THEMES, type PracticeRange as Range } from '../lib/practice'
import { storedChoice as stored, writeStorage } from '../lib/storage'
import type { PracticeItem, PracticeKind, PracticePhase, PracticeResult, PracticeStats, PracticeTheme } from '../lib/types'

type Kind = 'all' | PracticeKind
type Phase = 'all' | PracticePhase
type Theme = 'all' | PracticeTheme

const KINDS: { value: Kind; label: string }[] = [
  { value: 'all', label: 'All' }, { value: 'blunder', label: 'Blunders' },
  { value: 'mistake', label: 'Mistakes' }, { value: 'miss', label: 'Misses' },
]
const RANGES: [Range, string][] = [['all', 'All games, due first'], ['today', 'Today’s games'], ['7', 'Last 7 days'], ['30', 'Last 30 days']]
const PHASES: [Phase, string][] = [['all', 'All phases'], ['opening', 'Opening'], ['middlegame', 'Middlegame'], ['endgame', 'Endgame']]

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
  const [index, setIndex] = useState(0)
  const [round, setRound] = useState(0)
  const [score, setScore] = useState({ right: 0, done: 0 })
  const [stats, setStats] = useState<PracticeStats | null>(null)

  const queue = useQuery({
    queryKey: ['practice', account?.id, kind, range, phase, theme],
    queryFn: () => api.practice(account!.id, kind === 'all' ? undefined : kind, range === 'all' ? 10 : 30, since(range, nowTs()), phase === 'all' ? undefined : phase, theme === 'all' ? undefined : theme),
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
  // Bumped whenever the session starts over, so a refill that lands later does not reset it again.
  const session = useRef(0)
  const restart = () => {
    session.current++
    setRound(value => value + 1); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
    void qc.resetQueries({ queryKey: ['practice', account?.id] })
  }
  const choose = <T extends string>(key: string, current: T, set: (value: T) => void) => (value: T) => {
    // Re-selecting the active choice keeps the session (a Tabs click on the selected tab).
    if (value === current) return
    session.current++
    writeStorage(key, value); set(value); setIndex(0); setScore({ right: 0, done: 0 }); setStats(null)
  }
  const next = useCallback(() => { setIndex(value => value + 1) }, [])

  // Background review completion marks queues stale centrally. An empty batch can become ready;
  // active and completed batches stay in place until the user chooses the next round.
  const { data: queueData, isStale, isFetching, isError, refetch } = queue
  useEffect(() => {
    if (!queueData || queueData.items.length > 0 || !isStale || isFetching || isError) return
    const started = session.current
    void refetch().then(result => {
      if (session.current === started && !result.error) { setIndex(0); setStats(null) }
    })
  }, [queueData, isStale, isFetching, isError, refetch])

  const updateStats = (result: PracticeResult, signal: AbortSignal) => {
    if (kind === 'all' && phase === 'all' && range === 'all' && theme === 'all') { setStats(result.stats); return }
    // The answer response contains account totals. Fetch only filtered stats without replacing
    // the active batch (recent-game queues are shuffled by the server).
    void api.practice(account!.id, kind === 'all' ? undefined : kind, 1, since(range, nowTs()), phase === 'all' ? undefined : phase, theme === 'all' ? undefined : theme)
      .then(result => { if (!signal.aborted) setStats(result.stats) })
      .catch(() => { /* Keep the last known stats; the answer and schedule were already saved. */ })
  }
  const finish = useCallback((result: PracticeResult) => {
    setScore(value => ({ right: value.right + (result.correct ? 1 : 0), done: value.done + 1 }))
  }, [])

  if (loading) return <Loading />
  if (!account) {
    return <div className="appear"><PageHeader title="Practice" />
      <Empty title="Connect an account first" action={<Link to="/accounts"><Button variant="primary">Connect account</Button></Link>}>
        Practice uses positions from your own reviewed games.
      </Empty></div>
  }

  const header = (
    <PageHeader title="Practice"
      description="Positions from your games where a clearly better move was there. Find it; the ones you miss come back."
      actions={<>
        <Select label="Game phase" value={phase} onChange={v => choose<Phase>('ci.practicePhase', phase, setPhase)(v as Phase)}>
          {PHASES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </Select>
        <Select label="Tactical theme" value={theme} onChange={v => choose<Theme>('ci.practiceTheme', theme, setTheme)(v as Theme)}>
          <option value="all">All themes</option>
          {THEMES.filter(([value]) => value === theme || (queue.data?.theme_counts?.[value] ?? 0) > 0).map(([value, label]) => <option key={value} value={value}>{label} ({queue.data?.theme_counts?.[value] ?? 0})</option>)}
        </Select>
        <Select label="Which games" value={range} onChange={v => choose<Range>('ci.practiceRange', range, setRange)(v as Range)}>
          {RANGES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
        </Select>
        <Tabs label="Mistake type" value={kind} onChange={choose<Kind>('ci.practiceKind', kind, setKind)} options={KINDS} />
        <Switch compact label="Play out the line" checked={fullLine} onChange={v => {
          writeStorage('ci.practiceLine', v ? 'on' : 'off'); setFullLine(v)
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
        {range === 'all' && shownStats.next_due_at && !shownStats.due ? ` · next review ${dueIn(shownStats.next_due_at, nowTs())}` : ''}
      </div>
    </Card>
  ) : null

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
                    : shownStats?.next_due_at ? `The next review is due ${dueIn(shownStats.next_due_at, nowTs())}.` : 'You have answered every position.'}
                </Empty>}
          </Card></div>
        ) : (
          <PracticePuzzle key={`${kind}:${phase}:${theme}:${range}:${round}:${item.game_id}:${item.ply}`}
            account={account} item={item} index={index} total={items.length} fullLine={fullLine}
            progress={progress} onNext={next} onAnswered={updateStats} onFinished={finish} />
        )}
    </div>
  )
}
