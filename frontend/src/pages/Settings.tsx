import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import clsx from 'clsx'
import { Check, Save } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { PageHeader, Button, Card, ErrorNote, Input, Loading, Tabs, Switch } from '../components/ui'
import { api } from '../lib/api'
import { ACCENT_PRESETS, BOARD_THEMES, DEFAULT_ACCENT, isHexColour, setAppearance, useAppearance, type Mode } from '../lib/theme'
import type { Settings } from '../lib/types'

function Row({ label, description, children }: { label: string; description?: string; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-2">
      <span className="min-w-0 max-w-sm"><span className="block text-sm font-medium">{label}</span>{description && <span className="block text-xs text-fg-2">{description}</span>}</span>
      {children}
    </div>
  )
}

function Appearance() {
  const { mode, accent, board } = useAppearance()
  const [hex, setHex] = useState(accent)
  const [prevAccent, setPrevAccent] = useState(accent)
  if (accent !== prevAccent) { setPrevAccent(accent); setHex(accent) }
  const choose = (value: string) => setAppearance({ accent: value.toLowerCase() })
  const preset = ACCENT_PRESETS.some(p => p.value === accent)
  return <Card title="Appearance" subtitle="Stored in this browser and applied immediately.">
    <Row label="Theme">
      <Tabs<Mode> label="Theme" value={mode} onChange={m => setAppearance({ mode: m })}
        options={[{ value: 'system', label: 'System' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }]} />
    </Row>
    <Row label="Cover colour" description="Used for the logo, primary buttons and highlights. Text in this colour is adjusted to stay readable.">
      <div className="flex flex-wrap items-center gap-2">
        <div role="group" aria-label="Cover colour presets" className="flex items-center gap-1.5">
          {ACCENT_PRESETS.map(p => (
            <button key={p.value} type="button" aria-pressed={accent === p.value} aria-label={p.name} title={p.name}
              onClick={() => choose(p.value)} style={{ background: p.value }}
              className={clsx('h-8 w-8 rounded-[5px] ring-offset-2 ring-offset-surface transition-shadow sm:h-6 sm:w-6',
                accent === p.value ? 'ring-2 ring-fg' : 'ring-1 ring-inset ring-black/10 hover:ring-2 hover:ring-line-strong')} />
          ))}
        </div>
        <label className={clsx('relative h-8 w-8 cursor-pointer overflow-hidden rounded-[5px] ring-offset-2 ring-offset-surface sm:h-6 sm:w-6',
          preset ? 'ring-1 ring-inset ring-line-strong' : 'ring-2 ring-fg')} title="Any colour" style={preset ? undefined : { background: accent }}>
          {preset && <span aria-hidden className="absolute inset-0 bg-[conic-gradient(#e5484d,#f5d90a,#30a46c,#0090ff,#8e4ec6,#e5484d)]" />}
          <input type="color" aria-label="Any cover colour" value={accent} onChange={e => choose(e.target.value)} className="absolute inset-0 cursor-pointer opacity-0" />
        </label>
        <Input aria-label="Cover colour hex" className="w-[5.5rem] font-mono tnum" maxLength={7} spellCheck={false} value={hex}
          aria-invalid={!isHexColour(hex)}
          onChange={e => { const v = e.target.value.trim(); setHex(v); if (isHexColour(v)) choose(v) }}
          onBlur={() => setHex(accent)} />
        {accent !== DEFAULT_ACCENT && <Button variant="ghost" onClick={() => choose(DEFAULT_ACCENT)}>Reset</Button>}
      </div>
    </Row>
    <Row label="Board">
      <Tabs label="Board colours" value={board} onChange={b => setAppearance({ board: b })}
        options={BOARD_THEMES.map(t => ({ value: t.value, label: <span className="flex items-center gap-1.5"><span aria-hidden className="board-swatch h-3.5 w-3.5" data-board={t.value} />{t.label}</span> }))} />
    </Row>
  </Card>
}

function NumberField({ label, description, value, onChange, min, max, step = 1, suffix }:
  { label: string; description?: string; value: number; onChange: (v: number) => void; min: number; max: number; step?: number; suffix?: string }) {
  // Keep what is typed, so a field can be cleared and retyped; only numbers reach the form.
  const [text, setText] = useState<string | null>(null)
  return (
    <label className="flex items-center justify-between gap-4 py-2">
      <span className="min-w-0"><span className="block text-sm font-medium">{label}</span>{description && <span className="block text-xs text-fg-2">{description}</span>}</span>
      <span className="flex shrink-0 items-center gap-2">
        <Input type="number" value={text ?? String(value)} min={min} max={max} step={step} aria-label={label}
          onChange={e => { setText(e.target.value); if (e.target.value.trim() !== '' && Number.isFinite(Number(e.target.value))) onChange(Number(e.target.value)) }}
          onBlur={() => setText(null)}
          className="w-24 text-right tnum" />
        <span className="w-6 text-xs text-fg-3">{suffix}</span>
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
        <Row label="Analysis depth" description="Deeper is more accurate but slower. Standard (16) is a good balance; changes apply to newly queued games.">
          <Tabs label="Analysis depth" value={presetValue} onChange={v => v !== 'custom' && set('analysis_depth', presets[v])}
            options={[...Object.entries(presets).sort(([, a], [, b]) => a - b).map(([k, d]) => ({ value: k, label: <>{k[0].toUpperCase()}{k.slice(1)}<span className="max-sm:hidden"> ({d})</span></> })), { value: 'custom', label: 'Custom' }]} />
        </Row>
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
          <Input value={form.contact} onChange={e => set('contact', e.target.value)} placeholder="you@example.com" aria-label="Contact" className="w-56" />
        </label>
        <Switch checked={form.auto_analyze_new} onChange={v => set('auto_analyze_new', v)} label="Default: analyse new games automatically" />
      </Card>
      <Appearance />
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
