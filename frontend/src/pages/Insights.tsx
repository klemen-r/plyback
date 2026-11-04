import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { BarChart3, FileText, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import FilterBar from '../components/FilterBar'
import { DEFAULT_FILTERS, restoreFilters, toInsightFilters, type FilterState } from '../lib/filters'
import InsightsView from '../components/InsightsView'
import { PageHeader, Button, Empty, ErrorNote, Loading, Progress } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { timeAgo } from '../lib/format'

const KEY = 'ci.insightFilters'
function loadFilters(): FilterState {
  try { return restoreFilters(JSON.parse(localStorage.getItem(KEY) ?? '{}')) } catch { return DEFAULT_FILTERS }
}

export default function InsightsPage() {
  const { account, loading } = useAccount()
  const [fs, setFs] = useState<FilterState>(loadFilters)
  const filters = toInsightFilters(fs)
  const qc = useQueryClient()
  const nav = useNavigate()
  const update = (v: FilterState) => { setFs(v); try { localStorage.setItem(KEY, JSON.stringify(v)) } catch { /* ignore */ } }

  const openings = useQuery({ queryKey: ['openings', account?.id], queryFn: () => api.openings(account!.id), enabled: !!account })
  const q = useQuery({
    queryKey: ['insights', account?.id, filters],
    queryFn: () => api.insights(account!.id, filters),
    enabled: !!account,
    refetchInterval: query => query.state.data?.stale ? 2000 : 15000,
    placeholderData: (prev, query) => query?.queryKey[1] === account?.id ? prev : undefined,
  })
  const analyze = useMutation({
    mutationFn: () => api.analyzeScope(account!.id, filters),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ['status'] }); qc.invalidateQueries({ queryKey: ['accounts'] }); qc.invalidateQueries({ queryKey: ['insights'] }) },
  })
  const report = useMutation({
    mutationFn: () => api.createReport(account!.id, filters),
    onSuccess: r => nav(`/reports/${r.id}`),
  })

  if (loading) return <Loading />
  if (!account) return <Empty icon={<BarChart3 className="h-10 w-10" />} title="Connect a Chess.com account first" action={<Link to="/accounts"><Button variant="primary">Connect account</Button></Link>} />

  const dq = q.data?.data_quality
  const missing = dq ? dq.games - dq.analyzed : 0
  return (
    <div className="appear">
      <PageHeader title="Account insights" subtitle={`Patterns across ${account.display_name}'s games, backed by engine analysis of every move.`}
        actions={<Button onClick={() => report.mutate()} loading={report.isPending}><FileText className="h-4 w-4" />Run full analysis & save report</Button>} />
      <FilterBar value={fs} onChange={update} openings={openings.data} />
      {(analyze.error || report.error) && <div className="mt-4"><ErrorNote error={analyze.error ?? report.error} /></div>}
      {dq && (
        <div className="mt-4 flex flex-wrap items-center justify-between gap-4 rounded-lg border border-line bg-surface px-4 py-3">
          <div className="min-w-[240px] flex-1">
            <div className="flex items-center justify-between text-sm">
              <span><b className="tnum">{dq.analyzed}</b> of <b className="tnum">{dq.games}</b> games analysed</span>
              <span className="text-xs text-fg-3">{q.isFetching ? 'updating…' : q.data?.generated_at ? `updated ${timeAgo(q.data.generated_at)}` : ''}</span>
            </div>
            <Progress value={dq.analyzed} max={dq.games} className="mt-2" />
          </div>
          {missing > 0 && (
            <Button variant="primary" size="sm" onClick={() => analyze.mutate()} loading={analyze.isPending}>
              <Sparkles className="h-4 w-4" />{analyze.data ? `Queued ${analyze.data.queued}` : `Analyse ${missing} remaining`}
            </Button>
          )}
        </div>
      )}
      <div className="mt-6">
        {q.isLoading ? <Loading label="Crunching your games…" /> : q.error ? <ErrorNote error={q.error} /> : q.data ? <InsightsView data={q.data} /> : null}
      </div>
    </div>
  )
}
