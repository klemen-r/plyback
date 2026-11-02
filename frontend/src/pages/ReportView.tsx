import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Cpu, Printer } from 'lucide-react'
import { Link, useParams } from 'react-router-dom'
import InsightsView from '../components/InsightsView'
import { PageHeader, Button, Card, ErrorNote, Loading, Progress } from '../components/ui'
import { api } from '../lib/api'
import { fmtDate, fmtDuration } from '../lib/format'

export default function ReportView() {
  const id = Number(useParams().id)
  const qc = useQueryClient()
  const q = useQuery({ queryKey: ['report', id], queryFn: () => api.report(id), refetchInterval: query => (query.state.data?.status === 'analyzing' ? 3000 : false) })
  const status = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 4000, enabled: q.data?.status === 'analyzing' })
  const finalize = useMutation({ mutationFn: () => api.finalizeReport(id), onSuccess: () => qc.invalidateQueries({ queryKey: ['report', id] }) })
  if (q.isLoading) return <Loading />
  if (q.error) return <ErrorNote error={q.error} />
  const r = q.data!
  const remaining = r.total_games - r.analyzed_games
  const perGame = status.data?.queue.avg_game_seconds
  const workers = status.data?.queue.workers || 1
  return (
    <div className="appear">
      <Link to="/reports" className="mb-3 inline-flex items-center gap-1 text-sm text-fg-2 hover:text-fg"><ArrowLeft className="h-4 w-4" />Reports</Link>
      <PageHeader title={r.title ?? `Report #${r.id}`}
        subtitle={r.status === 'ready' ? `Snapshot from ${fmtDate(r.completed_at, true)} · ${r.analyzed_games} of ${r.total_games} games analysed` : `Created ${fmtDate(r.created_at, true)}`}
        actions={r.status === 'ready' ? <Button size="sm" onClick={() => window.print()}><Printer className="h-4 w-4" />Print / PDF</Button> : undefined} />
      {r.status === 'analyzing' && (
        <Card>
          <div className="flex flex-wrap items-center gap-4">
            <Cpu className="h-6 w-6 animate-pulse text-accent" />
            <div className="min-w-[240px] flex-1">
              <div className="flex justify-between text-sm"><span>Analysing games in scope…</span>
                <span className="tabular text-fg-2">{r.analyzed_games}/{r.total_games}{perGame ? ` · ~${fmtDuration((remaining * perGame) / workers)} left` : ''}</span></div>
              <Progress value={r.analyzed_games} max={r.total_games} className="mt-2" />
              <p className="mt-2 text-xs text-fg-3">You can leave this page — the report is built automatically when analysis finishes. Already-analysed games are reused.</p>
            </div>
            <Button size="sm" onClick={() => finalize.mutate()} loading={finalize.isPending} disabled={r.analyzed_games === 0}>Build now with {r.analyzed_games} games</Button>
          </div>
        </Card>
      )}
      {r.status === 'failed' && <ErrorNote error={r.error ?? 'Report failed'} />}
      {finalize.error && <ErrorNote error={finalize.error} />}
      {r.status === 'ready' && r.report && <InsightsView data={r.report} />}
    </div>
  )
}
