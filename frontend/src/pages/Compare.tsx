import { PageHeader } from '../components/ui'
import { SERIES } from '../lib/charts'
import { useQuery } from '@tanstack/react-query'
import clsx from 'clsx'
import { GitCompare } from 'lucide-react'
import { useSearchParams } from 'react-router-dom'
import { BarChart } from '../components/charts'
import FilterBar from '../components/FilterBar'
import { DEFAULT_FILTERS, restoreFilters, toInsightFilters, type FilterState } from '../lib/filters'
import { Card, Empty, ErrorNote, Loading, Select } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { PHASE_LABEL } from '../lib/classification'
import { difference, knownAccount } from '../lib/compare'
import type { Insights } from '../lib/types'
import { insightPollInterval } from '../lib/insight-query'
import { platformName } from '../lib/platform'

interface Segment { accountId: number | null; filters: FilterState }

/** A selection kept in the address (`?a=` / `?b=`), so Back and reload return to it. */
function readSegment(raw: string | null, fallback: Segment): Segment {
  if (!raw) return fallback
  try {
    const v = JSON.parse(raw) as { account?: unknown; filters?: unknown }
    const id = Number(v.account)
    return { accountId: Number.isInteger(id) && id > 0 ? id : null, filters: restoreFilters(v.filters) }
  } catch { return fallback }
}

function useSegment(seg: Segment) {
  const f = toInsightFilters(seg.filters)
  return useQuery({ queryKey: ['insights', seg.accountId, f], queryFn: () => api.insights(seg.accountId!, f), enabled: !!seg.accountId, refetchInterval: insightPollInterval })
}

type Row = { label: string; a: number | null | undefined; b: number | null | undefined; unit?: string; better: 'higher' | 'lower'; digits?: number }

/** Causes from either selection, so an empty side does not hide the other's. Unknown stays null. */
function causes(a: Insights, b: Insights) {
  const keys = [...new Map([...a.priorities, ...b.priorities].filter(p => p.ep_lost_per_game !== undefined).map(p => [p.key, p.title])).entries()]
  const loss = (d: Insights, key: string) => {
    const v = d.priorities.find(x => x.key === key)?.ep_lost_per_game
    return d.data_quality.analyzed && v !== undefined && v !== null ? +(v * 100).toFixed(1) : null
  }
  return keys.map(([key, title]) => ({ area: title.split(' (')[0], A: loss(a, key), B: loss(b, key) }))
}

function metrics(a: Insights, b: Insights): Row[] {
  const ph = (d: Insights, k: 'opening' | 'middlegame' | 'endgame') => d.phases[k].user.accuracy
  // With no analysed games a per-game rate is unknown, not 0.
  const engine = (d: Insights, v: number | null | undefined) => (d.data_quality.analyzed ? v : null)
  const tag = (d: Insights, t: string) => engine(d, d.errors.tags.find(x => x.tag === t)?.per_game)
  return [
    { label: 'Games', a: a.overview.games, b: b.overview.games, better: 'higher', digits: 0 },
    { label: 'Score', a: a.overview.score, b: b.overview.score, unit: '%', better: 'higher', digits: 1 },
    { label: 'Accuracy', a: a.overview.accuracy, b: b.overview.accuracy, unit: '%', better: 'higher', digits: 1 },
    { label: 'Avg. centipawn loss', a: a.overview.acpl, b: b.overview.acpl, better: 'lower', digits: 1 },
    { label: 'Blunders / game', a: engine(a, a.overview.per_game?.blunder), b: engine(b, b.overview.per_game?.blunder), better: 'lower', digits: 2 },
    { label: 'Mistakes / game', a: engine(a, a.overview.per_game?.mistake), b: engine(b, b.overview.per_game?.mistake), better: 'lower', digits: 2 },
    { label: 'Performance rating', a: a.overview.performance_rating, b: b.overview.performance_rating, better: 'higher', digits: 0 },
    ...(['opening', 'middlegame', 'endgame'] as const).map(k => ({ label: `${PHASE_LABEL[k]} accuracy`, a: ph(a, k), b: ph(b, k), unit: '%', better: 'higher' as const, digits: 1 })),
    { label: 'Hanging pieces / game', a: tag(a, 'hanging_piece'), b: tag(b, 'hanging_piece'), better: 'lower', digits: 2 },
    { label: 'Ignored threats / game', a: tag(a, 'threat_ignored'), b: tag(b, 'threat_ignored'), better: 'lower', digits: 2 },
    { label: 'Conversion rate', a: a.advantage.conversion_rate, b: b.advantage.conversion_rate, unit: '%', better: 'higher', digits: 0 },
    { label: 'Punish rate', a: a.missed.punish_rate, b: b.missed.punish_rate, unit: '%', better: 'higher', digits: 0 },
    { label: 'Games in time trouble', a: a.time.time_trouble?.share, b: b.time.time_trouble?.share, unit: '%', better: 'lower', digits: 0 },
  ]
}

function SegmentPicker({ label, seg, onChange, color }: { label: string; seg: Segment; onChange: (s: Segment) => void; color: string }) {
  const { accounts } = useAccount()
  return (
    <Card title={<span className="flex items-center gap-2"><span className="h-3 w-3 rounded-full" style={{ background: color }} />{label}</span>}
      actions={accounts.length > 1 ? (
        <Select label={`${label} account`} value={String(seg.accountId ?? '')} onChange={v => onChange({ ...seg, accountId: Number(v) })} className="h-8 text-xs">
          {accounts.map(a => <option key={a.id} value={a.id}>{a.display_name} · {platformName(a.platform)}</option>)}
        </Select>) : undefined}>
      <FilterBar value={seg.filters} onChange={f => onChange({ ...seg, filters: f })} />
    </Card>
  )
}

export default function Compare() {
  const { account, accounts, loading } = useAccount()
  const [sp, setSp] = useSearchParams()
  const a = readSegment(sp.get('a'), { accountId: null, filters: { ...DEFAULT_FILTERS, period: '30' } })
  const b = readSegment(sp.get('b'), { accountId: null, filters: { ...DEFAULT_FILTERS, period: 'all' } })
  const save = (key: 'a' | 'b') => (s: Segment) => {
    const next = new URLSearchParams(sp)
    next.set(key, JSON.stringify({ account: s.accountId, filters: s.filters }))
    setSp(next, { replace: true })
  }
  const setA = save('a'), setB = save('b')
  const segA = { ...a, accountId: knownAccount(a.accountId, accounts) ?? account?.id ?? null }
  const segB = { ...b, accountId: knownAccount(b.accountId, accounts) ?? account?.id ?? null }
  const qa = useSegment(segA)
  const qb = useSegment(segB)
  if (loading) return <Loading />
  if (!accounts.length) return <Empty icon={<GitCompare className="h-10 w-10" />} title="Connect an account to compare segments" />

  const rows = qa.data && qb.data ? metrics(qa.data, qb.data) : []
  return (
    <div className="appear space-y-6">
      <PageHeader title="Compare" description="Put two selections side by side: recent form vs. all time, blitz vs. rapid, White vs. Black, or two accounts." />
      <div className="grid gap-4 lg:grid-cols-2">
        <SegmentPicker label="Selection A" seg={segA} onChange={setA} color={SERIES.you} />
        <SegmentPicker label="Selection B" seg={segB} onChange={setB} color={SERIES.opp} />
      </div>
      {qa.error ? <ErrorNote error={qa.error} /> : qb.error ? <ErrorNote error={qb.error} /> : (qa.isLoading || qb.isLoading) ? <Loading /> : qa.data && qb.data && (
        <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_480px]">
          <Card title="Side by side" bodyClass="p-0">
            <table className="w-full text-sm">
              <thead><tr className="border-b border-line text-left text-[12.5px] text-fg-3"><th className="px-5 py-2 font-medium">Metric</th><th className="px-3 py-2 text-right font-medium">A</th><th className="px-3 py-2 text-right font-medium">B</th><th className="px-5 py-2 text-right font-medium">A − B</th></tr></thead>
              <tbody>
                {rows.map(r => {
                  const d = difference(r.a, r.b, r.digits ?? 1, r.better, r.unit)
                  return (
                    <tr key={r.label} className="border-b border-line/50 last:border-0">
                      <td className="px-5 py-2">{r.label}</td>
                      <td className="px-3 py-2 text-right tnum">{r.a !== null && r.a !== undefined ? `${r.a.toFixed(r.digits ?? 1)}${r.unit ?? ''}` : '—'}</td>
                      <td className="px-3 py-2 text-right tnum">{r.b !== null && r.b !== undefined ? `${r.b.toFixed(r.digits ?? 1)}${r.unit ?? ''}` : '—'}</td>
                      <td className={clsx('px-5 py-2 text-right font-semibold tnum', d.tone === 'good' ? 'text-good' : d.tone === 'bad' ? 'text-bad' : 'text-fg-3')}>{d.text}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
            <p className="px-5 py-3 text-xs text-fg-3">Analysed games: A {qa.data.data_quality.analyzed}, B {qb.data.data_quality.analyzed}. Differences on small samples are often noise.</p>
          </Card>
          <div className="space-y-6">
            <Card title="Accuracy by phase">
              <BarChart data={(['opening', 'middlegame', 'endgame'] as const).map(k => ({ phase: PHASE_LABEL[k], A: qa.data!.phases[k].user.accuracy, B: qb.data!.phases[k].user.accuracy }))}
                x="phase" unit="%" series={[{ key: 'A', name: 'A', color: SERIES.you }, { key: 'B', name: 'B', color: SERIES.opp }]} />
            </Card>
            <Card title="Win chance lost per game, by cause">
              <BarChart horizontal height={300} labelWidth={150}
                data={causes(qa.data, qb.data)} x="area" unit="%" series={[{ key: 'A', name: 'A', color: SERIES.you }, { key: 'B', name: 'B', color: SERIES.opp }]} />
            </Card>
          </div>
        </div>
      )}
    </div>
  )
}
