import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import clsx from 'clsx'
import { Pause, Play, RotateCcw, Trash2 } from 'lucide-react'
import { Link } from 'react-router-dom'
import { PageHeader, Button, Card, Empty, ErrorNote, Loading, Progress, Figure } from '../components/ui'
import { api } from '../lib/api'
import { fmtDate, fmtDuration, timeAgo } from '../lib/format'

const PRIORITY: Record<number, string> = { 100: 'manual', 50: 'new game', 30: 'report', 10: 'history' }

export default function ActivityPage() {
  const qc = useQueryClient()
  const q = useQuery({ queryKey: ['queue'], queryFn: api.queue, refetchInterval: 2000 })
  const ev = useQuery({ queryKey: ['events'], queryFn: () => api.events(80), refetchInterval: 5000 })
  const act = useMutation({ mutationFn: api.queueAction, onSuccess: () => { qc.invalidateQueries({ queryKey: ['queue'] }); qc.invalidateQueries({ queryKey: ['status'] }) } })
  if (q.isLoading) return <Loading />
  if (q.error) return <ErrorNote error={q.error} />
  const d = q.data!
  return (
    <div className="appear space-y-6">
      <PageHeader title="Activity" subtitle="Background analysis queue and synchronisation log. Everything here survives restarts."
        actions={<>
          {d.paused ? <Button variant="primary" onClick={() => act.mutate('resume')}><Play className="h-4 w-4" />Resume</Button>
            : <Button onClick={() => act.mutate('pause')}><Pause className="h-4 w-4" />Pause</Button>}
          {d.failed > 0 && <Button onClick={() => act.mutate('retry-failed')}><RotateCcw className="h-4 w-4" />Retry failed</Button>}
          {d.queued > 0 && <Button variant="ghost" onClick={() => act.mutate('clear')}><Trash2 className="h-4 w-4" />Clear queue</Button>}
        </>} />
      {d.engine_error && <ErrorNote error={d.engine_error} />}
      {act.error && <ErrorNote error={act.error} />}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
        <Figure label="Queued" value={d.queued} />
        <Figure label="Running" value={d.running} detail={`${d.workers} worker${d.workers === 1 ? '' : 's'}`} />
        <Figure label="Completed (7 days)" value={d.done} />
        <Figure label="Avg. per game" value={d.avg_game_seconds ? `${d.avg_game_seconds}s` : '—'} />
        <Figure label="Time remaining" value={d.paused && d.queued ? 'Paused' : d.eta_seconds ? fmtDuration(d.eta_seconds) : '—'} />
      </div>
      <Card title="Queue" subtitle={[d.engine, d.paused ? 'Paused' : null].filter(Boolean).join(' · ') || undefined} bodyClass="p-2">
        {d.jobs.length ? d.jobs.map(j => (
          <Link key={j.id} to={`/games/${j.game_id}`} className="flex items-center gap-4 rounded-lg px-3 py-2 text-sm hover:bg-subtle">
            <span className={clsx('h-2 w-2 shrink-0 rounded-full', j.status === 'running' ? 'animate-pulse bg-accent' : 'bg-fg-3')} />
            <span className="min-w-0 flex-1 truncate">{j.white_username} – {j.black_username} <span className="text-xs text-fg-3">{fmtDate(j.end_time)} · {j.time_class}</span></span>
            {j.status === 'running' && j.total ? <span className="flex w-40 items-center gap-2"><Progress value={j.progress ?? 0} max={j.total} /><span className="text-xs tnum text-fg-2">{j.progress}/{j.total}</span></span>
              : <span className="text-xs text-fg-3">{PRIORITY[j.priority] ?? `p${j.priority}`} · d{j.depth}</span>}
          </Link>
        )) : <Empty title="Queue is empty">Nothing is waiting to be reviewed.</Empty>}
        {d.queued > d.jobs.filter(j => j.status === 'queued').length && <p className="px-3 py-2 text-xs text-fg-3">…and {d.queued - d.jobs.filter(j => j.status === 'queued').length} more</p>}
      </Card>
      {d.failed_jobs.length > 0 && (
        <Card title="Failed" bodyClass="p-2">
          {d.failed_jobs.map(j => (
            <Link key={j.id} to={`/games/${j.game_id}`} className="block rounded-lg px-3 py-2 text-sm hover:bg-subtle">
              {j.white_username} – {j.black_username}<div className="text-xs text-bad">{j.error}</div>
            </Link>
          ))}
        </Card>
      )}
      <Card title="Log" bodyClass="p-0">
        <div className="max-h-[420px] overflow-y-auto">
          {(ev.data ?? []).map(e => (
            <div key={e.id} className="flex gap-3 border-b border-line/50 px-5 py-2 text-sm last:border-0">
              <span className="w-24 shrink-0 text-xs text-fg-3">{timeAgo(e.ts)}</span>
              <span className={clsx('w-16 shrink-0 text-xs font-medium capitalize', e.level === 'error' ? 'text-bad' : e.level === 'warning' ? 'text-warn' : 'text-fg-3')}>{e.source}</span>
              <span className="text-fg/90">{e.message}</span>
            </div>
          ))}
          {!ev.data?.length && <Empty title="Nothing logged yet" />}
        </div>
      </Card>
    </div>
  )
}
