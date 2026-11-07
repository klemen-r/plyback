import { SERIES } from '../lib/charts'
import clsx from 'clsx'
import { AlertTriangle, ChevronDown, ChevronRight, ExternalLink, Target, TrendingDown, TrendingUp } from 'lucide-react'
import { memo, useMemo, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { PHASE_LABEL, REASON_LABEL } from '../lib/classification'
import { cap, fmtDate, moveLabel, moveNumber, num, pct } from '../lib/format'
import { localHours } from '../lib/hours'
import type { Color, Example, Insights, OpeningFamily, Priority, Resource } from '../lib/types'
import { BarChart, LineChart, WDLBar } from './charts'
import PositionCard, { MiniBoard } from './Evidence'
import { Card, Confidence, Empty, Badge, Tabs, Figure } from './ui'

const SECTIONS = [
  ['priorities', 'Priorities'], ['trend', 'Rating & trend'], ['openings', 'Openings'], ['phases', 'Phases'],
  ['errors', 'Mistake patterns'], ['time', 'Time'], ['advantage', 'Advantage'], ['recurring', 'Recurring'], ['results', 'Results & habits'],
] as const

function Resources({ items }: { items: Resource[] }) {
  if (!items.length) return null
  return (
    <div className="flex flex-wrap gap-2">
      {items.map(r => (
        <a key={r.url + r.title} href={r.url} target="_blank" rel="noreferrer"
          className="inline-flex items-center gap-1.5 rounded-lg border border-line bg-subtle px-2.5 py-1.5 text-xs text-fg/90 hover:border-accent/60 hover:text-fg">
          <ExternalLink className="h-3 w-3 text-fg-3" />{r.title}
        </a>
      ))}
    </div>
  )
}

function Examples({ items, limit = 4 }: { items: Example[]; limit?: number }) {
  if (!items.length) return <p className="text-sm text-fg-3">No examples in this selection.</p>
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4">
      {items.slice(0, limit).map(e => <PositionCard key={`${e.game_id}-${e.ply}`} ex={e} />)}
    </div>
  )
}

function Compare({ user, opp, unit = '', better = 'lower', digits = 2 }: { user: number | null | undefined; opp: number | null | undefined; unit?: string; better?: 'lower' | 'higher'; digits?: number }) {
  if (user === null || user === undefined) return <span className="text-fg-3">—</span>
  const worse = opp !== null && opp !== undefined && (better === 'lower' ? user > opp * 1.1 : user < opp * 0.9)
  const nicer = opp !== null && opp !== undefined && (better === 'lower' ? user < opp * 0.9 : user > opp * 1.1)
  return (
    <span className="tnum">
      <span className={clsx('font-semibold', worse && 'text-bad', nicer && 'text-good')}>{user.toFixed(digits)}{unit}</span>
      {opp !== null && opp !== undefined && <span className="ml-1 text-xs text-fg-3">vs {opp.toFixed(digits)}{unit}</span>}
    </span>
  )
}

function PriorityCard({ p, open, onToggle }: { p: Priority; open: boolean; onToggle: () => void }) {
  const ratio = p.vs_opponents ?? 1
  return (
    <div className={clsx('rounded-lg border bg-surface transition-colors', open ? 'border-accent/50' : 'border-line')}>
      <button onClick={onToggle} className="flex w-full items-start gap-4 p-4 text-left">
        <div className={clsx('flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-sm font-semibold',
          p.rank === 1 ? 'bg-bad/20 text-bad' : p.rank && p.rank <= 3 ? 'bg-warn/15 text-warn' : 'bg-hover text-fg-2')}>
          {p.rank ?? '–'}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-semibold">{p.title}</h3>
            <Confidence c={p.confidence} n={p.errors} />
            {ratio >= 1.25 && <Badge className="bg-bad/15 text-bad"><TrendingDown className="h-3 w-3" />Worse than opponents</Badge>}
            {ratio <= 0.8 && p.ep_lost_per_game !== undefined && <Badge className="bg-accent/15 text-accent"><TrendingUp className="h-3 w-3" />Better than opponents</Badge>}
          </div>
          <p className="mt-1 text-sm text-fg-2">{p.summary}</p>
          {p.detail && <p className="mt-1 text-sm text-fg/80">{p.detail}</p>}
        </div>
        {p.ep_lost_per_game !== undefined && (
          <div className="hidden w-36 shrink-0 text-right sm:block">
            <div className="text-xl font-semibold tnum">{(p.ep_lost_per_game * 100).toFixed(0)}<span className="text-sm text-fg-2">%</span></div>
            <div className="text-[11px] text-fg-3">win chance lost / game</div>
          </div>
        )}
        {open ? <ChevronDown className="mt-1 h-5 w-5 shrink-0 text-fg-3" /> : <ChevronRight className="mt-1 h-5 w-5 shrink-0 text-fg-3" />}
      </button>
      {open && (
        <div className="space-y-4 border-t border-line p-4">
          <div>
            <div className="mb-2 text-[13px] font-medium text-fg-2">Evidence from your games</div>
            <Examples items={p.examples} />
          </div>
          {p.resources.length > 0 && (
            <div>
              <div className="mb-2 text-[13px] font-medium text-fg-2">Free training resources</div>
              <Resources items={p.resources} />
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function Priorities({ data }: { data: Insights }) {
  const ranked = data.priorities.filter(p => p.rank)
  const rest = data.priorities.filter(p => !p.rank)
  const [open, setOpen] = useState<string | null>(ranked[0]?.key ?? null)
  if (!ranked.length) {
    return <Card><Empty icon={<Target className="h-8 w-8" />} title="Not enough analysed games yet">
      Priorities appear once enough of your games have been analysed to separate real patterns from noise.</Empty></Card>
  }
  return (
    <div className="space-y-3">
      {ranked.map(p => <PriorityCard key={p.key} p={p} open={open === p.key} onToggle={() => setOpen(open === p.key ? null : p.key)} />)}
      {rest.length > 0 && (
        <p className="px-1 text-xs text-fg-3">
          Not ranked (too few occurrences to judge): {rest.map(r => r.title).join(', ')}.
        </p>
      )}
    </div>
  )
}

function OpeningTable({ families, color }: { families: OpeningFamily[]; color: Color }) {
  const [open, setOpen] = useState<string | null>(null)
  if (!families.length) return <p className="text-sm text-fg-3">No games as {color}.</p>
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[720px] text-sm">
        <thead>
          <tr className="border-b border-line text-left text-[12.5px] text-fg-3">
            <th className="py-2 font-medium">Opening</th>
            <th className="py-2 text-right font-medium">Games</th>
            <th className="w-40 py-2 pl-4 font-medium">Result</th>
            <th className="py-2 text-right font-medium">Score</th>
            <th className="py-2 text-right font-medium" title="Accuracy of your non-book moves in the opening phase">Opening acc.</th>
            <th className="py-2 text-right font-medium" title="Your average winning chances after move 10">After move 10</th>
            <th className="py-2 text-right font-medium" title="Mistakes + blunders in the opening per game">Errors/game</th>
          </tr>
        </thead>
        <tbody>
          {families.slice(0, 25).map(f => (
            <FragmentRows key={f.family} f={f} open={open === f.family} toggle={() => setOpen(open === f.family ? null : f.family)} color={color} />
          ))}
        </tbody>
      </table>
    </div>
  )
}

function FragmentRows({ f, open, toggle, color }: { f: OpeningFamily; open: boolean; toggle: () => void; color: Color }) {
  return (
    <>
      <tr className="cursor-pointer border-b border-line/60 hover:bg-subtle/60" onClick={toggle}>
        <td className="py-2.5 pr-2">
          <div className="flex items-center gap-2">
            {open ? <ChevronDown className="h-4 w-4 text-fg-3" /> : <ChevronRight className="h-4 w-4 text-fg-3" />}
            <span className="font-medium">{f.family}</span>
            {f.flag === 'weakness' && <Badge className="bg-bad/15 text-bad">weak spot</Badge>}
            {f.flag === 'strength' && <Badge className="bg-accent/15 text-accent">strength</Badge>}
            {f.confidence === 'insufficient' && <span className="text-[11px] text-fg-3">few games</span>}
          </div>
        </td>
        <td className="py-2.5 text-right tnum">{f.games}</td>
        <td className="py-2.5 pl-4"><WDLBar wins={f.wins} draws={f.draws} losses={f.losses} /></td>
        <td className="py-2.5 text-right font-semibold tnum">{pct(f.score)}</td>
        <td className="py-2.5 text-right tnum">{pct(f.opening_accuracy, 1)}</td>
        <td className={clsx('py-2.5 text-right tnum', f.exit_win !== null && (f.exit_win < 40 ? 'text-bad' : f.exit_win > 60 ? 'text-good' : ''))}>{pct(f.exit_win)}</td>
        <td className="py-2.5 text-right tnum">{num(f.opening_errors_per_game, 2)}</td>
      </tr>
      {open && (
        <tr className="border-b border-line/60 bg-subtle/40">
          <td colSpan={7} className="p-4">
            <div className="grid gap-4 lg:grid-cols-2">
              <div>
                <div className="mb-2 text-[13px] font-medium text-fg-2">Variations</div>
                <table className="w-full text-xs">
                  <tbody>
                    {f.variations.map(v => (
                      <tr key={v.name} className="border-b border-line/40 last:border-0">
                        <td className="py-1.5 pr-2">{v.name}</td>
                        <td className="py-1.5 text-right tnum text-fg-2">{v.games}</td>
                        <td className="w-24 py-1.5 pl-3"><WDLBar wins={v.wins} draws={v.draws} losses={v.losses} /></td>
                        <td className="py-1.5 text-right tnum">{pct(v.score)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="space-y-3">
                <div>
                  <div className="mb-2 text-[13px] font-medium text-fg-2">Recent games</div>
                  <div className="flex flex-wrap gap-2">
                    {f.recent_game_ids.map(id => <Link key={id} to={`/games/${id}`} className="rounded-md bg-hover px-2 py-1 text-xs text-accent hover:underline">Game #{id}</Link>)}
                    <Link to={`/games?opening=${encodeURIComponent(f.family)}&color=${color}`} className="rounded-md px-2 py-1 text-xs text-fg-2 hover:text-fg">All games →</Link>
                  </div>
                </div>
                <div>
                  <div className="mb-2 text-[13px] font-medium text-fg-2">Study it</div>
                  <Resources items={f.resources} />
                </div>
              </div>
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

function Openings({ data }: { data: Insights }) {
  const [color, setColor] = useState<Color>('white')
  const o = data.openings[color]
  return (
    <Card title="Opening performance" subtitle="Grouped by opening family (matched by position, so transpositions count). Score includes all games; accuracy and evaluation need analysed games."
      actions={<Tabs value={color} onChange={setColor} options={[{ value: 'white', label: 'As White' }, { value: 'black', label: 'As Black' }]} />}>
      <OpeningTable families={o.families} color={color} />
      {o.first_moves.length > 0 && (
        <div className="mt-6">
          <div className="mb-2 text-[13px] font-medium text-fg-2">{color === 'white' ? 'Your first move' : 'Your reply to White’s first move'}</div>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
            {o.first_moves.slice(0, 8).map(m => (
              <div key={m.move} className="rounded-lg border border-line bg-subtle px-3 py-2">
                <div className="flex items-center justify-between text-sm"><span className="font-mono font-semibold">{m.move}</span><span className="tnum text-fg-2">{m.games} g · {pct(m.score)}</span></div>
                <WDLBar wins={m.wins} draws={m.draws} losses={m.losses} className="mt-2" />
              </div>
            ))}
          </div>
        </div>
      )}
    </Card>
  )
}

function Phases({ data }: { data: Insights }) {
  const ph = data.phases
  const chart = (['opening', 'middlegame', 'endgame'] as const).map(k => ({
    phase: PHASE_LABEL[k], You: ph[k].user.accuracy, Opponents: ph[k].opponent.accuracy,
  }))
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title="Accuracy by game phase" subtitle="Average move accuracy (non-book moves), you vs. your opponents in the same games.">
        <BarChart data={chart} x="phase" series={[{ key: 'You', name: 'You', color: SERIES.you }, { key: 'Opponents', name: 'Opponents', color: SERIES.opp }]} unit="%" />
      </Card>
      <Card title="Error rates by phase" subtitle="Per 100 moves. Red = clearly worse than opponents.">
        <div className="overflow-x-auto">
        <table className="w-full min-w-[480px] text-sm">
          <thead><tr className="text-left text-[12.5px] text-fg-3"><th className="py-1.5 font-medium">Phase</th><th className="text-right font-medium">Moves</th><th className="text-right font-medium">Blunders</th><th className="text-right font-medium">Mistakes</th><th className="text-right font-medium">Win% lost / game</th></tr></thead>
          <tbody>
            {(['opening', 'middlegame', 'endgame'] as const).map(k => (
              <tr key={k} className="border-t border-line/60">
                <td className="py-2"><div className="flex items-center gap-2">{PHASE_LABEL[k]} <Confidence c={ph[k].confidence} /></div></td>
                <td className="text-right tnum text-fg-2">{ph[k].user.moves}</td>
                <td className="text-right"><Compare user={ph[k].user.blunders_per_100} opp={ph[k].opponent.blunders_per_100} digits={1} /></td>
                <td className="text-right"><Compare user={ph[k].user.mistakes_per_100} opp={ph[k].opponent.mistakes_per_100} digits={1} /></td>
                <td className="text-right"><Compare user={ph[k].ep_lost_per_game * 100} opp={ph[k].opp_ep_lost_per_game * 100} digits={0} unit="%" /></td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
        {ph.endgame_types.length > 0 && (
          <div className="mt-5">
            <div className="mb-2 text-[13px] font-medium text-fg-2">Endgames by type</div>
            <table className="w-full text-sm">
              <tbody>
                {ph.endgame_types.map(t => (
                  <tr key={t.type} className="border-t border-line/60">
                    <td className="py-1.5 capitalize">{t.type} endgames</td>
                    <td className="text-right tnum text-fg-2">{t.games} games</td>
                    <td className="text-right"><Compare user={t.user.accuracy} opp={t.opponent.accuracy} better="higher" digits={1} unit="%" /></td>
                    <td className="pl-2 text-right"><Confidence c={t.confidence} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  )
}

function Errors({ data }: { data: Insights }) {
  const e = data.errors
  const [open, setOpen] = useState<string | null>(null)
  const rows = e.tags.filter(t => t.count > 0 && !['tactical'].includes(t.tag))
  return (
    <Card title="What goes wrong" subtitle={`${e.total_errors} mistakes & blunders analysed — ${pct(e.tactical_share)} tactical, ${pct(e.positional_share)} positional. Categories overlap: one error can have several causes.`}>
      <div className="mb-4 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Figure label="Errors per game" value={num(e.errors_per_game, 2)} detail={`opponents ${num(e.opp_errors_per_game, 2)}`} tone={e.errors_per_game > e.opp_errors_per_game * 1.1 ? 'bad' : undefined} />
        {Object.entries(e.hung_pieces).sort((a, b) => b[1] - a[1]).slice(0, 3).map(([piece, n]) => (
          <Figure key={piece} label={`${cap(piece)}s hung`} value={n} />
        ))}
      </div>
      <div className="divide-y divide-line/60">
        {rows.map(t => (
          <div key={t.tag}>
            <button className="flex w-full items-center gap-3 py-2.5 text-left text-sm hover:bg-subtle/40" onClick={() => setOpen(open === t.tag ? null : t.tag)}>
              {open === t.tag ? <ChevronDown className="h-4 w-4 text-fg-3" /> : <ChevronRight className="h-4 w-4 text-fg-3" />}
              <span className="flex-1 font-medium">{t.label}</span>
              <span className="w-28 text-right text-xs"><Compare user={t.per_game} opp={t.opp_per_game} /> <span className="text-fg-3">/game</span></span>
              <span className="w-24 text-right font-semibold tnum">{(t.ep_lost_per_game * 100).toFixed(0)}% <span className="text-xs font-normal text-fg-3">lost</span></span>
              <span className="hidden w-36 text-right sm:block"><Confidence c={t.confidence} n={t.count} /></span>
            </button>
            {open === t.tag && <div className="pb-4 pl-7"><Examples items={t.examples} limit={3} /></div>}
          </div>
        ))}
      </div>
    </Card>
  )
}

function Time({ data }: { data: Insights }) {
  const t = data.time
  if (!t.games) return <Card title="Time management"><Empty title="No clock data">Time statistics need analysed games with clock information (live games from Chess.com include it).</Empty></Card>
  const tt = t.time_trouble!
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Figure label="Games in time trouble" value={pct(tt.share)} detail={`${tt.games} of ${t.games} · score ${pct(tt.score)}`} hint="Clock fell below 10% of the base time (min. 10s)" />
        <Figure label="Errors / 100 moves when low" value={num(tt.errors_per_100_low, 1)} detail={`normal clock: ${num(tt.errors_per_100_normal, 1)}`}
          tone={(tt.errors_per_100_low ?? 0) > (tt.errors_per_100_normal ?? 0) * 1.5 ? 'bad' : undefined} />
        <Figure label="Losses on time" value={t.timeouts?.losses_on_time ?? 0} detail={`${pct(t.timeouts?.share_of_losses)} of losses · ${t.timeouts?.in_non_losing_position ?? 0} not losing`} tone={(t.timeouts?.in_non_losing_position ?? 0) > 2 ? 'warn' : undefined} />
        <Figure label="Clock left at end" value={pct(t.clock_left_at_end?.win)} detail={`in wins · losses: ${pct(t.clock_left_at_end?.loss)}`} />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="Clock usage" subtitle="Average % of base time remaining by move number.">
          <LineChart data={t.clock_curve ?? []} x="move" yUnit="%" yDomain={[0, 100]}
            series={[{ key: 'user', name: 'You', color: SERIES.you }, { key: 'opponent', name: 'Opponents', color: SERIES.opp, dashed: true }]} />
        </Card>
        <Card title="Speed vs. quality" subtitle="Errors per 100 moves by time spent (excludes time-trouble moves).">
          <BarChart data={(t.speed_vs_quality ?? []).map(s => ({ ...s, label: `${s.label}` }))} x="label"
            series={[{ key: 'errors_per_100', name: 'Errors / 100 moves', color: SERIES.warn }, { key: 'blunders_per_100', name: 'Blunders / 100', color: SERIES.bad }]} />
          <div className="mt-2 grid grid-cols-4 gap-2 text-center text-[11px] text-fg-3">
            {(t.speed_vs_quality ?? []).map(s => <div key={s.label}>{s.range}<br />{s.moves} moves</div>)}
          </div>
        </Card>
      </div>
      {t.avg_time_per_move && (
        <Card title="Average time per move">
          <div className="grid grid-cols-3 gap-3">
            {Object.entries(t.avg_time_per_move).map(([ph, v]) => (
              <div key={ph} className="rounded-lg bg-subtle px-3 py-2 text-sm">
                <div className="text-xs text-fg-3">{PHASE_LABEL[ph]}</div>
                <Compare user={v.user} opp={v.opponent} unit="s" better="higher" digits={1} />
              </div>
            ))}
          </div>
        </Card>
      )}
    </div>
  )
}

function Advantage({ data }: { data: Insights }) {
  const a = data.advantage
  const m = data.missed
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Figure label="Winning positions converted" value={pct(a.conversion_rate)} detail={`${a.converted} of ${a.winning_positions} games with ≥80% win chance`} />
        <Figure label="Lost positions saved" value={pct(a.save_rate)} detail={`${a.saved} of ${a.losing_positions} games with ≤20%`} />
        <Figure label="Opponent errors punished" value={pct(m.punish_rate)} detail={`${m.punished} of ${m.opponent_errors} mistakes/blunders`} />
        <Figure label="Missed forced mates" value={m.missed_mates} detail="mate in ≤3 available" />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="How your losses happen" subtitle={`The single costliest move in each of ${a.losses_analyzed} analysed losses.`}>
          {a.how_you_lose.length ? (
            <div className="space-y-2">
              {a.how_you_lose.slice(0, 8).map(h => (
                <div key={h.key} className="flex items-center gap-3 text-sm">
                  <span className="w-48 shrink-0 truncate">{h.phase === 'gradual' ? 'Gradual (no single big error)' : h.phase === 'time' ? 'Ran out of time' : `${PHASE_LABEL[h.phase] ?? h.phase}: ${KIND_LABEL[h.kind] ?? h.kind}`}</span>
                  <div className="h-2 flex-1 overflow-hidden rounded-full bg-hover"><div className="h-full rounded-full bg-bad/80" style={{ width: `${h.share ?? 0}%` }} /></div>
                  <span className="w-16 text-right tnum text-fg-2">{h.count} · {pct(h.share)}</span>
                </div>
              ))}
            </div>
          ) : <p className="text-sm text-fg-3">No analysed losses.</p>}
        </Card>
        <Card title="Advantages that slipped away" subtitle="Games where you reached ≥80% winning chances but didn't win.">
          {a.lost_advantages.length ? (
            <div className="max-h-80 space-y-2 overflow-y-auto pr-1">
              {a.lost_advantages.map(l => (
                <Link key={l.game_id} to={`/games/${l.game_id}${l.culprit ? `?ply=${l.culprit.ply}` : ''}`} className="flex items-center justify-between gap-3 rounded-lg bg-subtle px-3 py-2 text-sm hover:bg-hover">
                  <span className="min-w-0"><span className="font-medium">vs {l.opponent}</span> <span className="text-xs text-fg-3">{fmtDate(l.end_time)}</span>
                    {l.culprit && <span className="block truncate text-xs text-fg-2">Turning point: {moveLabel(l.culprit.ply, l.culprit.san, l.culprit.fen)} (best {l.culprit.best_san})</span>}</span>
                  <span className="shrink-0 text-right text-xs"><span className="text-good">peak {l.peak_win.toFixed(0)}%</span><br /><span className={l.outcome === 'loss' ? 'text-bad' : 'text-fg-2'}>{l.outcome} · {REASON_LABEL[l.reason] ?? l.reason}</span></span>
                </Link>
              ))}
            </div>
          ) : <p className="text-sm text-fg-3">None — every winning position was converted.</p>}
        </Card>
      </div>
      <Card title="Missed opportunities" subtitle="Your opponent erred, and your reply let the advantage go.">
        <Examples items={m.examples} limit={8} />
      </Card>
    </div>
  )
}

const KIND_LABEL: Record<string, string> = {
  allowed_mate: 'allowed mate', hanging_piece: 'hung a piece', allows_fork: 'allowed a fork', threat_ignored: 'ignored a threat',
  missed_punish: 'missed a punish', loses_material: 'lost material', positional: 'positional error', other: 'other', timeout: 'timeout',
  'no single decisive error': '',
}

function Recurring({ data }: { data: Insights }) {
  if (!data.recurring.length) return <Card title="Recurring mistakes"><Empty title="No repeated mistakes found">We look for positions where you went wrong in two or more different games.</Empty></Card>
  return (
    <Card title="Recurring mistakes" subtitle="The same position, reached in different games, where you chose a weaker move. These are the cheapest points to win back.">
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {data.recurring.slice(0, 9).map(r => (
          <div key={r.fen} className="flex gap-3 rounded-lg border border-line bg-subtle p-3">
            <div className="w-32 shrink-0"><MiniBoard fen={r.fen} orientation={r.color} played={r.played[0]?.san} best={r.best_san} /></div>
            <div className="min-w-0 flex-1 text-sm">
              <div className="font-semibold">{r.occurrences}× in {r.games.length} games</div>
              <div className="text-xs text-fg-2">Move {moveNumber(r.fen, r.move_number)} · {PHASE_LABEL[r.phase]}</div>
              <div className="mt-1.5 text-xs">You played {r.played.map(p => <span key={p.san} className="mr-1 font-mono text-bad">{p.san}{p.count > 1 ? `×${p.count}` : ''}</span>)}</div>
              <div className="text-xs">Better: <span className="font-mono text-accent">{r.best_san}</span></div>
              <div className="mt-2 flex flex-wrap gap-2 text-xs">
                {r.games.slice(-3).map(id => <Link key={id} className="text-accent hover:underline" to={`/games/${id}`}>#{id}</Link>)}
                <a href={r.lichess_url ?? undefined} target="_blank" rel="noreferrer" className="text-fg-2 hover:text-fg">Lichess ↗</a>
              </div>
            </div>
          </div>
        ))}
      </div>
    </Card>
  )
}

function Results({ data }: { data: Insights }) {
  const r = data.results
  const s = data.sessions
  const hours = useMemo(() => localHours(r), [r])
  const reasons = (obj: Record<string, number>) => Object.entries(obj).sort((a, b) => b[1] - a[1])
  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-3">
        <Card title="How games end">
          <div className="space-y-3 text-sm">
            {([['Wins', r.wins_by, 'text-good'], ['Losses', r.losses_by, 'text-bad'], ['Draws', r.draws_by, 'text-fg-2']] as const).map(([label, obj, tone]) => (
              <div key={label}>
                <div className={clsx('mb-1 text-[13px] font-medium', tone)}>{label}</div>
                <div className="flex flex-wrap gap-1.5">{reasons(obj).map(([k, v]) => <Badge key={k}>{REASON_LABEL[k] ?? k}: {v}</Badge>)}{!reasons(obj).length && <span className="text-xs text-fg-3">none</span>}</div>
              </div>
            ))}
          </div>
        </Card>
        <Card title="Vs. opponent strength" subtitle="Actual score vs. Elo expectation.">
          <table className="w-full text-sm">
            <tbody>
              {r.by_opponent.map(b => (
                <tr key={b.label} className="border-b border-line/50 last:border-0">
                  <td className="py-1.5">{b.label}</td>
                  <td className="text-right tnum text-fg-2">{b.games}</td>
                  <td className="text-right"><Compare user={b.score} opp={b.expected} better="higher" digits={0} unit="%" /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
        <Card title="Sessions & tilt" subtitle={`${s.sessions} sessions (games < 30 min apart).`}>
          <div className="space-y-2 text-sm">
            {(['win', 'loss', 'draw'] as const).map(k => (
              <div key={k} className="flex justify-between"><span className="text-fg-2">After a {k}</span><span className="tnum">{pct(s.after[k].score)} <span className="text-xs text-fg-3">({s.after[k].games})</span></span></div>
            ))}
            <div className="border-t border-line pt-2" />
            {Object.entries(s.by_game_in_session).map(([k, v]) => (
              <div key={k} className="flex justify-between"><span className="text-fg-2">Game {k} of a session</span><span className="tnum">{pct(v.score)} <span className="text-xs text-fg-3">({v.games})</span></span></div>
            ))}
            {s.tilt && (
              <div className={clsx('mt-2 rounded-lg px-3 py-2 text-xs', s.tilt.significant ? 'bg-warn/10 text-warn' : 'bg-subtle text-fg-2')}>
                {s.tilt.significant ? <AlertTriangle className="mr-1 inline h-3.5 w-3.5" /> : null}
                After a loss you score {s.tilt.score_after_loss}% vs. {s.tilt.baseline}% overall ({s.tilt.difference > 0 ? '+' : ''}{s.tilt.difference} pts){s.tilt.significant ? ' — statistically meaningful.' : ' — within normal variation.'}
              </div>
            )}
          </div>
        </Card>
      </div>
      <Card title="Score by time of day" subtitle="Your local time; hours with fewer than 5 games are hidden.">
        <BarChart data={hours} x="hour" series={[{ key: 'score', name: 'Score %', color: SERIES.opp }]} unit="%" height={200} />
      </Card>
    </div>
  )
}

function Trend({ data }: { data: Insights }) {
  const tcs = Object.keys(data.ratings).sort((a, b) => data.ratings[b].games - data.ratings[a].games)
  const [chosen, setTc] = useState(tcs[0] ?? '')
  // The filters can remove the chosen time control; fall back to the one with most games.
  const tc = data.ratings[chosen] ? chosen : tcs[0] ?? ''
  const rating = data.ratings[tc]
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title="Rating" subtitle={rating ? `${rating.start} → ${rating.end} (${rating.change >= 0 ? '+' : ''}${rating.change}) · peak ${rating.peak}` : 'No rated games'}
        actions={tcs.length > 1 ? <Tabs value={tc} onChange={setTc} options={tcs.map(t => ({ value: t, label: cap(t) }))} /> : undefined}>
        {rating ? <LineChart data={rating.points} x="t" xFormat={v => fmtDate(Number(v))} series={[{ key: 'rating', name: 'Rating', color: SERIES.you }]} zoomable unitLabel="games" /> : <Empty title="No rated games in this selection" />}
      </Card>
      <Card title="Accuracy & results over time" subtitle="Per week or month. Accuracy uses analysed games only.">
        <LineChart data={data.trend} x="period" yUnit="%" yDomain={[0, 100]}
          series={[{ key: 'accuracy', name: 'Accuracy', color: SERIES.you }, { key: 'score', name: 'Score', color: SERIES.opp, dashed: true }]} />
      </Card>
    </div>
  )
}

export function KPIs({ data }: { data: Insights }) {
  const o = data.overview
  const blunders = o.per_game?.blunder
  const oppBlunders = o.opp_per_game?.blunder
  return (
    <div className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
      <Figure label="Games" value={o.games} detail={`${o.wins}W · ${o.draws}D · ${o.losses}L`} />
      <Figure label="Score" value={pct(o.score)} detail={`White ${pct(o.white.score)} · Black ${pct(o.black.score)}`} />
      <Figure label="Accuracy" value={pct(o.accuracy, 1)} detail={`opponents ${pct(o.opp_accuracy, 1)}`} tone={o.accuracy && o.opp_accuracy ? (o.accuracy >= o.opp_accuracy ? 'good' : 'bad') : undefined}
        hint="Our Stockfish-based accuracy (Lichess formula). Not the same number as Chess.com's CAPS." />
      <Figure label="Blunders / game" value={num(blunders, 2)} detail={`opponents ${num(oppBlunders, 2)}`} tone={blunders && oppBlunders ? (blunders > oppBlunders * 1.1 ? 'bad' : blunders < oppBlunders * 0.9 ? 'good' : undefined) : undefined} />
      <Figure label="Performance" value={o.performance_rating ?? '—'} detail={`avg. opponent ${o.avg_opponent_rating ?? '—'}`} hint="Linear performance rating: average opponent + 400 × (W − L) / N" />
      <Figure label="Conversion" value={pct(data.advantage.conversion_rate)} detail={`${data.advantage.converted}/${data.advantage.winning_positions} winning positions`} />
    </div>
  )
}

export default memo(function InsightsView({ data, header }: { data: Insights; header?: ReactNode }) {
  const dq = data.data_quality
  if (!dq.games) return <Card><Empty title="No games in this selection">Try a wider time period or remove some filters.</Empty></Card>
  return (
    <div className="space-y-6">
      {header}
      {dq.notes.length > 0 && (
        <div className="rounded-lg border border-warn/30 bg-warn/5 px-4 py-3 text-sm text-warn">
          {dq.notes.map(n => <div key={n}>{n}</div>)}
        </div>
      )}
      <nav className="sticky top-[6.5rem] z-10 -mx-1 flex gap-1 overflow-x-auto rounded-lg border border-line bg-surface/80 p-1 backdrop-blur-xl backdrop-saturate-150">
        {SECTIONS.map(([id, label]) => (
          <a key={id} href={`#${id}`} onClick={e => { e.preventDefault(); document.getElementById(id)?.scrollIntoView({ behavior: 'smooth' }) }}
            className="shrink-0 rounded-lg px-3 py-1.5 text-xs font-medium text-fg-2 hover:bg-subtle hover:text-fg">{label}</a>
        ))}
      </nav>
      <KPIs data={data} />
      <Section id="priorities" title="Where to improve first" sub="Ranked by how many winning chances each area costs you per game, weighted by how you compare with opponents of similar strength.">
        <Priorities data={data} />
      </Section>
      <Section id="trend" title="Rating & trend"><Trend data={data} /></Section>
      <Section id="openings" title="Openings"><Openings data={data} /></Section>
      <Section id="phases" title="Game phases"><Phases data={data} /></Section>
      <Section id="errors" title="Mistake patterns"><Errors data={data} /></Section>
      <Section id="time" title="Time management" sub={`Based on ${data.time.games} analysed games with clock data.`}><Time data={data} /></Section>
      <Section id="advantage" title="Advantage & missed chances"><Advantage data={data} /></Section>
      <Section id="recurring" title="Recurring positions"><Recurring data={data} /></Section>
      <Section id="results" title="Results & habits"><Results data={data} /></Section>
    </div>
  )
})

function Section({ id, title, sub, children }: { id: string; title: string; sub?: string; children: ReactNode }) {
  return (
    <section id={id} className="scroll-mt-28">
      <div className="mb-3">
        <h2 className="text-lg font-semibold">{title}</h2>
        {sub && <p className="text-sm text-fg-2">{sub}</p>}
      </div>
      {children}
    </section>
  )
}
