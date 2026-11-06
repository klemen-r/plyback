import { useMutation, useQueryClient } from '@tanstack/react-query'
import { ArrowUpRight, RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Avatar } from '../components/Layout'
import { Badge, Button, Card, ErrorNote, Input, PageHeader, Progress, Section, Select, Switch } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { fmtDate, timeAgo } from '../lib/format'
import type { Account } from '../lib/types'

function monthsAgo(n: number): string {
  const d = new Date()
  d.setUTCDate(1)
  d.setUTCMonth(d.getUTCMonth() - n + 1)
  return `${d.getUTCFullYear()}/${String(d.getUTCMonth() + 1).padStart(2, '0')}`
}

export function AddAccountForm({ onAdded }: { onAdded?: (a: Account) => void }) {
  const [username, setUsername] = useState('')
  const [range, setRange] = useState('all')
  const [history, setHistory] = useState(true)
  const qc = useQueryClient()
  const add = useMutation({
    mutationFn: () => api.addAccount({ username: username.trim(), import_since: range === 'all' ? null : monthsAgo(Number(range)), analyze_history: history, auto_analyze: true }),
    onSuccess: a => { qc.invalidateQueries({ queryKey: ['accounts'] }); setUsername(''); onAdded?.(a) },
  })
  return (
    <form onSubmit={e => { e.preventDefault(); if (username.trim()) add.mutate() }}>
      <label htmlFor="username" className="text-[14px] font-medium">Chess.com username</label>
      <div className="mt-2 flex flex-col gap-2 sm:flex-row">
        <Input id="username" value={username} onChange={e => setUsername(e.target.value)} placeholder="e.g. hikaru" autoComplete="off" autoFocus className="h-9 flex-1" />
        <Select label="History to import" value={range} onChange={setRange} className="h-9">
          <option value="all">All history</option>
          <option value="1">This month</option>
          <option value="3">Last 3 months</option>
          <option value="6">Last 6 months</option>
          <option value="12">Last 12 months</option>
          <option value="24">Last 2 years</option>
        </Select>
        <Button type="submit" variant="primary" className="h-9" loading={add.isPending} disabled={!username.trim()}>Connect</Button>
      </div>
      <div className="mt-2 border-t border-line">
        <Switch checked={history} onChange={setHistory} label="Review imported history in the background"
          description="New games are always reviewed. Turn this off to review older games only when you ask for them." />
      </div>
      {add.error && <div className="mt-1"><ErrorNote error={add.error} /></div>}
      <p className="mt-2 text-[12.5px] text-fg-3">Uses Chess.com's public API. No password or Chess.com login is needed.</p>
    </form>
  )
}

function AccountCard({ a }: { a: Account }) {
  const qc = useQueryClient()
  const { select, account } = useAccount()
  const refresh = () => qc.invalidateQueries({ queryKey: ['accounts'] })
  const patch = useMutation({ mutationFn: (b: Parameters<typeof api.patchAccount>[1]) => api.patchAccount(a.id, b), onSuccess: refresh })
  const sync = useMutation({ mutationFn: () => api.syncAccount(a.id), onSuccess: refresh })
  const del = useMutation({ mutationFn: () => api.deleteAccount(a.id), onSuccess: refresh })
  const [confirm, setConfirm] = useState(false)
  const ratings = Object.entries(a.ratings).filter(([, r]) => r.rating)
  return (
    <Card padded={false}>
      <div className="flex flex-wrap items-start gap-4 p-5">
        <Avatar src={a.avatar} name={a.username} size={40} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[16px] font-semibold tracking-[-0.01em]">{a.title && <span className="mr-1.5 text-bad">{a.title}</span>}{a.display_name}</span>
            {account?.id === a.id ? <Badge>Selected</Badge> : <button className="text-[13px] text-fg-2 hover:text-fg" onClick={() => select(a.id)}>Select</button>}
            <a href={a.profile_url} target="_blank" rel="noreferrer" className="flex items-center gap-0.5 text-[13px] text-fg-3 hover:text-fg">Chess.com<ArrowUpRight className="h-3 w-3" /></a>
          </div>
          <div className="mt-1 text-[13px] text-fg-2">
            {ratings.map(([tc, r]) => `${tc[0].toUpperCase()}${tc.slice(1)} ${r.rating}`).join(' · ') || 'No ratings'}
            <span className="text-fg-3"> · joined {fmtDate(a.joined)} · {a.import_since ? `importing since ${a.import_since}` : 'full history'}</span>
          </div>
          <div className="mt-4 max-w-md">
            <div className="flex justify-between text-[12.5px] text-fg-2"><span className="tnum">{a.analyzed} of {a.games} games reviewed</span><span className="tnum text-fg-3">{a.queued ? `${a.queued} queued` : ''}</span></div>
            <Progress value={a.analyzed} max={a.games} className="mt-1.5" />
          </div>
          <div className="mt-2 text-[12.5px]">
            {a.sync_status === 'syncing' ? <span className="text-fg-2">Syncing with Chess.com…</span>
              : a.sync_status === 'error' ? <span className="text-bad">Sync failed: {a.sync_error}</span>
                : <span className="text-fg-3">Last synced {timeAgo(a.last_sync_at)}</span>}
          </div>
        </div>
        <div className="flex gap-2">
          <Button size="sm" onClick={() => sync.mutate()} loading={sync.isPending || a.sync_status === 'syncing'}><RefreshCw className="h-3.5 w-3.5" />Sync</Button>
          {confirm
            ? <><Button size="sm" variant="danger" loading={del.isPending} onClick={() => del.mutate()}>Remove</Button><Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>Cancel</Button></>
            : <Button size="sm" variant="ghost" onClick={() => setConfirm(true)}>Remove…</Button>}
        </div>
      </div>
      {confirm && <div className="border-t border-line bg-bad/5 px-5 py-3 text-[13px]">Removing deletes this account's games and reviews from Plyback. Games shared with another connected account are kept.</div>}
      <div className="grid gap-x-10 border-t border-line px-5 md:grid-cols-3">
        <Switch checked={a.sync_enabled} disabled={patch.isPending} onChange={v => patch.mutate({ sync_enabled: v })} label="Sync automatically" description="Check Chess.com for new games" />
        <Switch checked={a.auto_analyze} disabled={patch.isPending} onChange={v => patch.mutate({ auto_analyze: v })} label="Review new games" description="As soon as they finish" />
        <Switch checked={a.analyze_history} disabled={patch.isPending} onChange={v => patch.mutate({ analyze_history: v })} label="Review history" description="Older games, in the background" />
      </div>
      {(patch.error || sync.error || del.error) && <div className="space-y-2 px-5 pb-4">
        {patch.error && <ErrorNote error={patch.error} />}
        {sync.error && <ErrorNote error={sync.error} />}
        {del.error && <ErrorNote error={del.error} />}
      </div>}
    </Card>
  )
}

export default function Accounts() {
  const { accounts } = useAccount()
  const nav = useNavigate()
  return (
    <div className="appear space-y-7">
      <PageHeader title="Accounts" description="Chess.com accounts connected to this Plyback server. Games sync automatically and are reviewed on this computer." />
      <Section title="Connect an account">
        <Card><AddAccountForm onAdded={() => { if (!accounts.length) nav('/') }} /></Card>
      </Section>
      {accounts.length > 0 && (
        <Section title="Connected">
          <div className="space-y-4">{accounts.map(a => <AccountCard key={a.id} a={a} />)}</div>
        </Section>
      )}
    </div>
  )
}
