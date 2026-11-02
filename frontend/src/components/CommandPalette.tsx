import { useQuery } from '@tanstack/react-query'
import clsx from 'clsx'
import { CornerDownLeft, Search } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { fmtDate } from '../lib/format'
import { TABS } from '../lib/nav'
import { ErrorNote, ResultMark, Spinner } from './ui'

interface Item { id: string; group: string; label: string; hint?: string; to: string; outcome?: 'win' | 'loss' | 'draw' | null }

/** Ctrl/⌘ K: jump to a page or search games by opponent or opening. */
export default function CommandPalette({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('')
  const [debounced, setDebounced] = useState('')
  const [activeIndex, setActive] = useState(0)
  const nav = useNavigate()
  const { account } = useAccount()
  const input = useRef<HTMLInputElement>(null)
  const listId = useId()
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    input.current?.focus()
    return () => previous?.focus()
  }, [])
  useEffect(() => { const t = setTimeout(() => setDebounced(q.trim()), 150); return () => clearTimeout(t) }, [q])

  const games = useQuery({
    queryKey: ['palette', account?.id, debounced],
    queryFn: () => api.games({ account_id: account?.id, q: debounced, page_size: 8 }),
    enabled: debounced.length >= 2,
  })

  const items: Item[] = useMemo(() => {
    const ql = q.toLowerCase()
    const pages = [...TABS, { to: '/accounts', label: 'Accounts' }, { to: '/methodology', label: 'Methodology' }]
      .filter(t => !ql || t.label.toLowerCase().includes(ql))
      .map(t => ({ id: `p${t.to}`, group: 'Go to', label: t.label, to: t.to }))
    const found = (debounced.length >= 2 ? games.data?.items ?? [] : []).map(g => {
      const opp = g.user_color === 'white' ? g.black : g.user_color === 'black' ? g.white : null
      return {
        id: `g${g.id}`, group: 'Games', to: `/games/${g.id}`, outcome: g.outcome,
        label: opp ? `vs ${opp.username}` : `${g.white.username} – ${g.black.username}`,
        hint: [g.opening.name, fmtDate(g.end_time)].filter(Boolean).join(' · '),
      }
    })
    return [...found, ...pages]
  }, [q, debounced, games.data])

  const active = Math.min(activeIndex, Math.max(0, items.length - 1))
  useEffect(() => { document.getElementById(`${listId}-${active}`)?.scrollIntoView({ block: 'nearest' }) }, [active, listId])
  const go = (it: Item | undefined) => { if (it) { nav(it.to); onClose() } }

  return (
    <div className="fixed inset-0 z-[70] flex items-start justify-center bg-black/30 px-4 pt-[12vh] backdrop-blur-[2px]" onMouseDown={onClose}>
      <div role="dialog" aria-modal="true" aria-label="Search" onMouseDown={e => e.stopPropagation()}
        onKeyDown={e => { if (e.key === 'Tab') { e.preventDefault(); input.current?.focus() } }}
        className="appear w-full max-w-[560px] overflow-hidden rounded-xl border border-line bg-surface shadow-[var(--shadow-pop)]">
        <div className="flex items-center gap-2.5 border-b border-line px-4">
          <Search className="h-4 w-4 text-fg-3" />
          <input ref={input} value={q} onChange={e => { setQ(e.target.value); setActive(0) }} placeholder="Search games by opponent or opening, or jump to a page"
            aria-label="Search" role="combobox" aria-expanded="true" aria-controls={listId}
            aria-activedescendant={items.length ? `${listId}-${active}` : undefined}
            className="h-12 flex-1 bg-transparent text-[14.5px] text-fg outline-none placeholder:text-fg-3"
            onKeyDown={e => {
              if (e.key === 'Escape') onClose()
              else if (e.key === 'ArrowDown') { e.preventDefault(); setActive(Math.min(Math.max(0, items.length - 1), active + 1)) }
              else if (e.key === 'ArrowUp') { e.preventDefault(); setActive(Math.max(0, active - 1)) }
              else if (e.key === 'Enter') go(items[active])
            }} />
          {games.isFetching && <Spinner />}
        </div>
        {games.error && <div className="p-2"><ErrorNote error={games.error} /></div>}
        <div id={listId} className="max-h-[360px] overflow-y-auto p-1.5" role="listbox" aria-label="Search results">
          {items.length === 0 && <div className="px-3 py-8 text-center text-[13px] text-fg-3">No matches.</div>}
          {items.map((it, i) => (
            <div key={it.id}>
              {(i === 0 || items[i - 1].group !== it.group) && <div className="px-2.5 pb-1 pt-2 text-[12px] text-fg-3">{it.group}</div>}
              <button id={`${listId}-${i}`} tabIndex={-1} type="button" role="option" aria-selected={i === active} onMouseEnter={() => setActive(i)} onClick={() => go(it)}
                className={clsx('flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-[14px]', i === active ? 'bg-hover text-fg' : 'text-fg-2')}>
                {it.group === 'Games' && <ResultMark outcome={it.outcome ?? null} />}
                <span className="truncate text-fg">{it.label}</span>
                {it.hint && <span className="truncate text-[12.5px] text-fg-3">{it.hint}</span>}
                {i === active && <CornerDownLeft className="ml-auto h-3.5 w-3.5 shrink-0 text-fg-3" />}
              </button>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
