import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { FileText, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import FilterBar from '../components/FilterBar'
import { DEFAULT_FILTERS, toInsightFilters, type FilterState } from '../lib/filters'
import { PageHeader, Button, Card, Empty, ErrorNote, Input, Loading, Badge, Progress } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { fmtDate } from '../lib/format'

export default function Reports() {
  const { account, loading } = useAccount()
  const qc = useQueryClient()
  const nav = useNavigate()
  const [fs, setFs] = useState<FilterState>(DEFAULT_FILTERS)
  const [title, setTitle] = useState('')
  const list = useQuery({
    queryKey: ['reports', account?.id], queryFn: () => api.reports(account!.id), enabled: !!account,
    refetchInterval: q => (q.state.data?.some(r => r.status === 'analyzing') ? 3000 : false),
  })
  const create = useMutation({ mutationFn: () => api.createReport(account!.id, toInsightFilters(fs), title || undefined), onSuccess: r => nav(`/reports/${r.id}`) })
  // Reports are frozen snapshots that cannot be recreated, so deleting one asks first.
  const [confirming, setConfirming] = useState<number | null>(null)
  // Each row tracks its own delete, so a slow one does not spin or close another row's confirmation.
  const [deleting, setDeleting] = useState<ReadonlySet<number>>(new Set())
  const del = useMutation({
    mutationFn: (id: number) => api.deleteReport(id),
    onMutate: id => setDeleting(s => new Set(s).add(id)),
    onSettled: (_data, _error, id) => {
      // A report already deleted elsewhere answers 404; refreshing still drops it from the list.
      qc.invalidateQueries({ queryKey: ['reports'] })
      setDeleting(s => { const next = new Set(s); next.delete(id); return next })
      setConfirming(c => (c === id ? null : c))
    },
  })
  const cancelDelete = (id: number) => { setConfirming(null); requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-report-delete="${id}"]`)?.focus()) }

  if (loading) return <Loading />
  if (!account) return <Empty icon={<FileText className="h-10 w-10" />} title="Connect a Chess.com account first" action={<Link to="/accounts"><Button variant="primary">Connect account</Button></Link>} />
  return (
    <div className="appear space-y-6">
      <PageHeader title="Account analysis reports" subtitle="A report analyses every game in the selected scope (reusing existing analyses) and saves a snapshot you can revisit." />
      <Card title="New report">
        <div className="space-y-3">
          <FilterBar value={fs} onChange={setFs} />
          <div className="flex flex-wrap gap-2">
            <Input value={title} onChange={e => setTitle(e.target.value)} maxLength={120} placeholder="Title (optional)" aria-label="Report title" className="min-w-48 flex-1" />
            <Button variant="primary" onClick={() => create.mutate()} loading={create.isPending}><Plus className="h-4 w-4" />Analyse & create report</Button>
          </div>
          {create.error && <ErrorNote error={create.error} />}
        </div>
      </Card>
      <Card title="Saved reports" bodyClass="p-2">
        {(list.error || del.error) && <ErrorNote error={list.error ?? del.error} />}
        {list.isLoading ? <Loading /> : list.data?.length ? list.data.map(r => (
          <div key={r.id} className="flex items-center gap-4 rounded-lg px-3 py-3 hover:bg-subtle">
            <FileText className="h-5 w-5 shrink-0 text-fg-3" />
            <Link to={`/reports/${r.id}`} className="min-w-0 flex-1">
              <div className="truncate font-medium">{r.title ?? `Report #${r.id}`}</div>
              <div className="text-xs text-fg-3">Created {fmtDate(r.created_at, true)} · {r.total_games} games</div>
              {r.status === 'analyzing' && <Progress value={r.analyzed_games} max={r.total_games} className="mt-2 max-w-sm" />}
            </Link>
            <Badge className={r.status === 'ready' ? 'bg-accent/15 text-accent' : r.status === 'failed' ? 'bg-bad/15 text-bad' : 'bg-accent-ink/20 text-accent-ink'}>
              {r.status === 'analyzing' ? `analysing ${r.analyzed_games}/${r.total_games}` : r.status}
            </Badge>
            {confirming === r.id || deleting.has(r.id)
              ? <span className="flex items-center gap-2" onKeyDown={e => { if (e.key === 'Escape' && !deleting.has(r.id)) cancelDelete(r.id) }}>
                  <Button size="sm" variant="danger" loading={deleting.has(r.id)} onClick={() => del.mutate(r.id)}>Delete</Button>
                  <Button size="sm" variant="ghost" autoFocus disabled={deleting.has(r.id)} onClick={() => cancelDelete(r.id)}>Cancel</Button>
                </span>
              : <Button size="sm" variant="ghost" data-report-delete={r.id} onClick={() => setConfirming(r.id)} aria-label="Delete report"><Trash2 className="h-4 w-4" /></Button>}
          </div>
        )) : <Empty title="No reports yet">Create one above — for example "last 90 days of blitz".</Empty>}
      </Card>
    </div>
  )
}
