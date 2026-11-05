import { useQuery } from '@tanstack/react-query'
import clsx from 'clsx'
import { Check, ChevronsUpDown, LogOut, Monitor, Moon, Plus, Search, Sun } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Link, NavLink, useLocation } from 'react-router-dom'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { fmtDuration } from '../lib/format'
import { TABS } from '../lib/nav'
import { setAppearance, useAppearance, type Mode } from '../lib/theme'
import CommandPalette from './CommandPalette'
import { Mark } from './Logo'
import { ErrorNote, Kbd } from './ui'



export function Avatar({ src, name, size = 22 }: { src?: string | null; name: string; size?: number }) {
  return src
    ? <img src={src} alt="" width={size} height={size} className="shrink-0 rounded-full object-cover" style={{ width: size, height: size }} />
    : <span className="flex shrink-0 items-center justify-center rounded-full bg-subtle text-[11px] font-medium text-fg-2" style={{ width: size, height: size }}>{name[0]?.toUpperCase()}</span>
}

function useDismiss(ref: React.RefObject<HTMLElement | null>, open: boolean, close: () => void) {
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) close() }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') close() }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey) }
  }, [ref, open, close])
}

function Menu({ trigger, children, align = 'left', label }: { trigger: (open: boolean) => ReactNode; children: (close: () => void) => ReactNode; align?: 'left' | 'right'; label: string }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const close = () => setOpen(false)
  useDismiss(ref, open, close)
  return (
    <div className="relative" ref={ref}>
      <button type="button" aria-haspopup="menu" aria-expanded={open} aria-label={label} onClick={() => setOpen(o => !o)}
        className="flex h-10 min-w-10 items-center justify-center gap-2 rounded-md px-1.5 text-[14px] transition-colors hover:bg-hover sm:h-auto sm:min-w-0 sm:py-1">
        {trigger(open)}
      </button>
      {open && (
        <div role="menu" className={clsx('appear absolute top-full z-50 mt-1.5 min-w-[220px] rounded-lg border border-line bg-surface p-1 shadow-[var(--shadow-pop)]',
          align === 'right' ? 'right-0' : 'left-0')}>
          {children(close)}
        </div>
      )}
    </div>
  )
}

function MenuItem({ children, onClick, to, active }: { children: ReactNode; onClick?: () => void; to?: string; active?: boolean }) {
  const cls = 'flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-[13.5px] text-fg hover:bg-hover'
  const inner = <>{children}{active && <Check className="ml-auto h-3.5 w-3.5 text-fg-2" />}</>
  return to
    ? <Link role="menuitem" to={to} onClick={onClick} className={cls}>{inner}</Link>
    : <button role="menuitem" type="button" onClick={onClick} className={cls}>{inner}</button>
}

function AccountMenu() {
  const { accounts, account, select, loading } = useAccount()
  // Hold the space while accounts load, so the header doesn't flash "Connect account".
  if (!account && loading) return <span className="skeleton h-6 w-24 rounded-md" aria-hidden />
  if (!account) {
    return <Link to="/accounts" className="flex items-center gap-1.5 rounded-md px-1.5 py-1 text-[14px] text-fg-2 hover:bg-hover hover:text-fg"><Plus className="h-4 w-4" />Connect account</Link>
  }
  return (
    <Menu label="Switch account" trigger={() => (
      <><Avatar src={account.avatar} name={account.username} /><span className="max-w-[160px] truncate font-medium">{account.display_name}</span><ChevronsUpDown className="h-3.5 w-3.5 text-fg-3" /></>
    )}>
      {close => (
        <>
          <div className="px-2 pb-1 pt-1.5 text-[12px] text-fg-3">Chess.com accounts</div>
          {accounts.map(a => (
            <MenuItem key={a.id} active={a.id === account.id} onClick={() => { select(a.id); close() }}>
              <Avatar src={a.avatar} name={a.username} size={18} /><span className="truncate">{a.display_name}</span>
            </MenuItem>
          ))}
          <div className="my-1 h-px bg-line" />
          <MenuItem to="/accounts" onClick={close}><Plus className="h-4 w-4 text-fg-2" />Add or manage accounts</MenuItem>
        </>
      )}
    </Menu>
  )
}

function EngineStatus() {
  const { data } = useQuery({ queryKey: ['status'], queryFn: api.status })
  if (!data) return null
  const q = data.queue
  const busy = q.running > 0 || q.queued > 0
  const state = data.engine.error ? 'error' : q.paused ? 'paused' : busy ? 'busy' : 'idle'
  const text = {
    error: 'Engine unavailable',
    paused: 'Analysis paused',
    busy: `Analysing · ${q.queued + q.running} left${q.eta_seconds ? ` · ~${fmtDuration(q.eta_seconds)}` : ''}`,
    idle: 'Up to date',
  }[state]
  return (
    <Link to="/activity" aria-label={text} className="flex h-10 min-w-10 items-center justify-center gap-2 rounded-md px-2 text-[13px] text-fg-2 transition-colors hover:bg-hover hover:text-fg md:h-auto md:min-w-0 md:py-1" title={data.engine.name ?? 'Stockfish'}>
      <span className={clsx('h-2 w-2 rounded-full', state === 'error' && 'bg-bad', state === 'paused' && 'bg-warn',
        state === 'busy' && 'animate-pulse bg-accent-ink', state === 'idle' && 'bg-good')} />
      <span className="tnum max-md:hidden">{text}</span>
    </Link>
  )
}

function UserMenu({ onLogout }: { onLogout?: () => void }) {
  const { mode } = useAppearance()
  const modes: { v: Mode; label: string; icon: typeof Sun }[] = [
    { v: 'light', label: 'Light', icon: Sun }, { v: 'dark', label: 'Dark', icon: Moon }, { v: 'system', label: 'System', icon: Monitor },
  ]
  const Icon = modes.find(m => m.v === mode)?.icon ?? Monitor
  return (
    <Menu label="Appearance and session" align="right" trigger={() => <Icon className="h-4 w-4 text-fg-2" />}>
      {close => (
        <>
          <div className="px-2 pb-1 pt-1.5 text-[12px] text-fg-3">Theme</div>
          {modes.map(m => (
            <MenuItem key={m.v} active={mode === m.v} onClick={() => { setAppearance({ mode: m.v }); close() }}>
              <m.icon className="h-4 w-4 text-fg-2" />{m.label}
            </MenuItem>
          ))}
          <div className="my-1 h-px bg-line" />
          <MenuItem to="/settings" onClick={close}>Settings</MenuItem>
          <MenuItem to="/methodology" onClick={close}>How the analysis works</MenuItem>
          {onLogout && (<><div className="my-1 h-px bg-line" /><MenuItem onClick={() => { close(); onLogout() }}><LogOut className="h-4 w-4 text-fg-2" />Sign out</MenuItem></>)}
        </>
      )}
    </Menu>
  )
}

export default function Layout({ children, onLogout }: { children: ReactNode; onLogout?: () => void }) {
  const { error: accountError } = useAccount()
  const [palette, setPalette] = useState(false)
  const loc = useLocation()
  const nav = useRef<HTMLElement>(null)
  useEffect(() => {
    window.scrollTo(0, 0)
    // On narrow screens the tab strip scrolls; keep the current tab visible.
    const active = nav.current?.querySelector<HTMLElement>('[aria-current=page]')
    if (active && nav.current && nav.current.scrollWidth > nav.current.clientWidth) {
      nav.current.scrollTo({ left: active.offsetLeft - (nav.current.clientWidth - active.offsetWidth) / 2 })
    }
  }, [loc.pathname])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setPalette(p => !p) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
  const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.platform)

  return (
    <div className="min-h-screen">
      <a href="#main" className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[60] focus:rounded-md focus:bg-surface focus:px-3 focus:py-2">Skip to content</a>
      <header className="sticky top-0 z-40 border-b border-line bg-surface/85 backdrop-blur-md supports-[backdrop-filter]:bg-surface/75">
        <div className="mx-auto flex h-14 max-w-[1280px] items-center gap-2 px-4 sm:px-6">
          <Link to="/" className="flex items-center gap-2 rounded-md px-1 py-1 text-[15px] font-semibold tracking-[-0.02em]" aria-label="Plyback home">
            <Mark size={20} /><span className="hidden sm:inline">Plyback</span>
          </Link>
          <span className="select-none text-[20px] font-extralight text-line-strong" aria-hidden>/</span>
          <AccountMenu />
          <div className="ml-auto flex items-center gap-1">
            <button type="button" onClick={() => setPalette(true)} aria-label="Search"
              className="mr-1 flex h-10 w-10 items-center justify-center gap-2 rounded-md border border-line bg-subtle text-[13px] text-fg-3 transition-colors hover:border-line-strong hover:text-fg-2 sm:h-8 sm:w-56 sm:justify-start sm:px-2.5">
              <Search className="h-3.5 w-3.5" /><span className="hidden sm:inline">Search games…</span>
              <span className="ml-auto hidden sm:inline"><Kbd>{isMac ? '⌘' : 'Ctrl'} K</Kbd></span>
            </button>
            <EngineStatus />
            <UserMenu onLogout={onLogout} />
          </div>
        </div>
        <nav ref={nav} aria-label="Main" className="mx-auto max-w-[1280px] overflow-x-auto px-2 sm:px-4 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          <div className="flex min-w-max">
            {TABS.map(t => (
              <NavLink key={t.to} to={t.to} end={t.end}
                className={({ isActive }) => clsx('relative px-2.5 pb-3 pt-1.5 text-[14px] transition-colors',
                  isActive ? 'text-fg after:absolute after:inset-x-2.5 after:-bottom-px after:h-0.5 after:rounded-full after:bg-fg' : 'text-fg-2 hover:text-fg')}>
                {t.label}
              </NavLink>
            ))}
          </div>
        </nav>
      </header>
      <main id="main" className="mx-auto w-full max-w-[1280px] px-4 pb-20 pt-6 sm:px-6 sm:pt-7">
        {!!accountError && <div className="mb-4"><ErrorNote error={accountError} /></div>}{children}
      </main>
      {palette && <CommandPalette onClose={() => setPalette(false)} />}
    </div>
  )
}
