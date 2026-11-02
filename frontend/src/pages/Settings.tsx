import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Save } from 'lucide-react'
import { useState } from 'react'
import { PageHeader, Button, Card, ErrorNote, Input, Loading, Tabs, Switch } from '../components/ui'
import { api } from '../lib/api'
import type { Settings } from '../lib/types'

function NumberField({ label, description, value, onChange, min, max, step = 1, suffix }:
  { label: string; description?: string; value: number; onChange: (v: number) => void; min: number; max: number; step?: number; suffix?: string }) {
  return (
    <label className="flex items-center justify-between gap-4 py-2">
      <span><span className="block text-sm font-medium">{label}</span>{description && <span className="block text-xs text-fg-2">{description}</span>}</span>
      <span className="flex items-center gap-2">
        <input type="number" value={value} min={min} max={max} step={step} onChange={e => onChange(Number(e.target.value))} aria-label={label}
          className="h-9 w-24 rounded-lg border border-line bg-subtle px-3 text-right text-sm tnum outline-none focus:border-accent" />
        {suffix && <span className="w-8 text-xs text-fg-3">{suffix}</span>}
      </span>
    </label>
  )
}

function AccessPassword() {
  const qc = useQueryClient()
  const session = useQuery({ queryKey: ['session'], queryFn: api.session })
  const [current, setCurrent] = useState('')
  const [password, setPassword] = useState('')
  const [confirmation, setConfirmation] = useState('')
  const save = useMutation({
    mutationFn: () => api.setPassword(current || null, password),
    onSuccess: state => {
      qc.setQueryData(['session'], state)
      setCurrent(''); setPassword(''); setConfirmation('')
    },
  })
  const protected_ = session.data?.password_set
  return <Card title="Access password" subtitle="Protect access to this server. Changing the password signs out other sessions.">
    <form className="space-y-3" onSubmit={e => { e.preventDefault(); if (password.length >= 8 && password === confirmation) save.mutate() }}>
      {protected_ && <label className="block text-sm">Current password<Input className="mt-1 block w-full" type="password" autoComplete="current-password" required value={current} onChange={e => setCurrent(e.target.value)} /></label>}
      <label className="block text-sm">New password<Input className="mt-1 block w-full" type="password" autoComplete="new-password" minLength={8} maxLength={256} required value={password} onChange={e => setPassword(e.target.value)} /></label>
      <label className="block text-sm">Confirm new password<Input className="mt-1 block w-full" type="password" autoComplete="new-password" required value={confirmation} onChange={e => setConfirmation(e.target.value)} /></label>
      <p className="text-xs text-fg-2">Use at least 8 characters.</p>
      {confirmation && password !== confirmation && <p role="status" className="text-sm text-bad">The passwords do not match.</p>}
      {save.error && <ErrorNote error={save.error} />}
      {save.isSuccess && <p role="status" className="text-sm text-good">Access password updated.</p>}
      <Button type="submit" loading={save.isPending} disabled={password.length < 8 || password !== confirmation || (!!protected_ && !current)}>{protected_ ? 'Change access password' : 'Set access password'}</Button>
    </form>
  </Card>
}

export default function SettingsPage() {
  const qc = useQueryClient()
  const s = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const status = useQuery({ queryKey: ['status'], queryFn: api.status })
  const [draft, setForm] = useState<Settings | null>(null)
  const form = draft ?? s.data
  const save = useMutation({
    mutationFn: (patch: Partial<Settings>) => api.saveSettings(patch),
    onSuccess: d => { qc.setQueryData(['settings'], d); setForm(d); qc.invalidateQueries({ queryKey: ['status'] }) },
  })
  if (s.error || status.error) return <ErrorNote error={s.error ?? status.error} />
  if (!form || !status.data) return <Loading />
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setForm({ ...form, [k]: v })
  const dirty = JSON.stringify(form) !== JSON.stringify(s.data)
  const presets = status.data.depth_presets
  const presetValue = Object.entries(presets).find(([, d]) => d === form.analysis_depth)?.[0] ?? 'custom'
  const cpus = status.data.cpu_count

  return (
    <div className="appear max-w-3xl space-y-6">
      <PageHeader title="Settings" actions={<Button variant="primary" disabled={!dirty} loading={save.isPending} onClick={() => save.mutate(form)}>{save.isSuccess && !dirty ? <Check className="h-4 w-4" /> : <Save className="h-4 w-4" />}Save changes</Button>} />
      {save.error && <ErrorNote error={save.error} />}
      <Card title="Engine" subtitle={status.data.engine.error ? undefined : `${status.data.engine.name ?? 'Stockfish'} · ${status.data.engine.path ?? ''}`}>
        {status.data.engine.error && <div className="mb-3"><ErrorNote error={status.data.engine.error} /></div>}
        <div className="flex items-center justify-between gap-4 py-2">
          <span><span className="block text-sm font-medium">Analysis depth</span><span className="block text-xs text-fg-2">Deeper is more accurate but slower. Standard (16) is a good balance; changes apply to newly queued games.</span></span>
          <Tabs value={presetValue} onChange={v => v !== 'custom' && set('analysis_depth', presets[v])}
            options={[...Object.entries(presets).map(([k, d]) => ({ value: k, label: `${k[0].toUpperCase()}${k.slice(1)} (${d})` })), { value: 'custom', label: 'Custom' }]} />
        </div>
        <NumberField label="Custom depth" value={form.analysis_depth} onChange={v => set('analysis_depth', v)} min={6} max={30} />
        <NumberField label="Engine lines (MultiPV)" description="2+ is needed to detect “only moves” (Great)" value={form.analysis_multipv} onChange={v => set('analysis_multipv', v)} min={1} max={5} />
        <NumberField label="Time cap per position" value={form.max_seconds_per_position} onChange={v => set('max_seconds_per_position', v)} min={0.2} max={60} step={0.5} suffix="s" />
        <NumberField label="Parallel workers" description={`Stockfish processes analysing games at once (this machine has ${cpus} logical CPUs)`} value={form.workers} onChange={v => set('workers', v)} min={1} max={cpus} />
        <NumberField label="Threads per worker" value={form.threads_per_worker} onChange={v => set('threads_per_worker', v)} min={1} max={cpus} />
        <NumberField label="Hash per worker" value={form.hash_mb} onChange={v => set('hash_mb', v)} min={16} max={4096} suffix="MB" />
        <NumberField label="Interactive depth" description="Depth for live analysis when exploring positions on the board" value={form.interactive_depth} onChange={v => set('interactive_depth', v)} min={8} max={32} />
      </Card>
      <Card title="Synchronisation">
        <NumberField label="Check for new games every" value={form.sync_interval_seconds} onChange={v => set('sync_interval_seconds', v)} min={60} max={86400} step={30} suffix="sec" />
        <NumberField label="…while actively playing" description="Used for 30 minutes after a new game was found" value={form.active_sync_interval_seconds} onChange={v => set('active_sync_interval_seconds', v)} min={30} max={3600} step={10} suffix="sec" />
        <label className="flex items-center justify-between gap-4 py-2">
          <span><span className="block text-sm font-medium">Contact (optional)</span><span className="block text-xs text-fg-2">Added to the User-Agent so Chess.com can reach you about API usage.</span></span>
          <input value={form.contact} onChange={e => set('contact', e.target.value)} placeholder="you@example.com" aria-label="Contact"
            className="h-9 w-56 rounded-lg border border-line bg-subtle px-3 text-sm outline-none placeholder:text-fg-3 focus:border-accent" />
        </label>
        <Switch checked={form.auto_analyze_new} onChange={v => set('auto_analyze_new', v)} label="Default: analyse new games automatically" />
      </Card>
      <AccessPassword />
      <Card title="About">
        <div className="grid grid-cols-2 gap-2 text-sm text-fg-2">
          <span>Version</span><span className="text-fg">{status.data.version} (analysis v{status.data.analysis_version})</span>
          <span>Games stored</span><span className="text-fg tnum">{status.data.games.toLocaleString()}</span>
          <span>Games analysed</span><span className="text-fg tnum">{status.data.analyzed.toLocaleString()}</span>
          <span>Database size</span><span className="text-fg tnum">{status.data.db_size ? `${(status.data.db_size / 1e6).toFixed(1)} MB` : '—'}</span>
        </div>
      </Card>
    </div>
  )
}
