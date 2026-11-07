import clsx from 'clsx'
import { AlertCircle, Loader2 } from 'lucide-react'
import type React from 'react'
import { useEffect, useRef } from 'react'
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode } from 'react'
import type { Confidence as Conf } from '../lib/types'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'

export function Button({ variant = 'secondary', size = 'md', loading, className, children, onClick, ref, ...rest }:
  ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: 'sm' | 'md' | 'icon'; loading?: boolean; ref?: React.Ref<HTMLButtonElement> }) {
  // Buttons that start work (they pass `loading`) are disabled only after the next render, so
  // the second click of a double-click would submit twice. Hold a short lock from the click
  // until the work shows as loading (then `disabled` takes over) or has finished.
  const lockedUntil = useRef(0)
  useEffect(() => { if (!loading) lockedUntil.current = 0 }, [loading])
  const click = (e: React.MouseEvent<HTMLButtonElement>) => {
    if (loading !== undefined) {
      // preventDefault also stops a second form submission from a submit button.
      if (e.timeStamp < lockedUntil.current) { e.preventDefault(); return }
      lockedUntil.current = e.timeStamp + 500
    }
    onClick?.(e)
  }
  return (
    <button
      type="button"
      {...rest}
      ref={ref}
      onClick={click}
      disabled={rest.disabled || loading}
      className={clsx(
        'inline-flex shrink-0 select-none items-center justify-center gap-1.5 whitespace-nowrap rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-50',
        size === 'sm' && 'h-9 px-2.5 text-[13px] sm:h-7',
        size === 'md' && 'h-10 px-3 text-[13.5px] sm:h-8',
        size === 'icon' && 'h-10 w-10 sm:h-8 sm:w-8',
        variant === 'primary' && 'bg-accent text-accent-fg hover:opacity-90',
        variant === 'secondary' && 'border border-line bg-surface text-fg shadow-[0_1px_0_rgb(0_0_0/0.02)] hover:bg-hover',
        variant === 'ghost' && 'text-fg-2 hover:bg-hover hover:text-fg',
        variant === 'danger' && 'border border-line bg-surface text-bad hover:border-bad/40 hover:bg-bad/5',
        className,
      )}
    >
      {loading && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
      {children}
    </button>
  )
}

export function Card({ children, className, padded = true, title, subtitle, actions, bodyClass }:
  { children: ReactNode; className?: string; padded?: boolean; title?: ReactNode; subtitle?: ReactNode; actions?: ReactNode; bodyClass?: string }) {
  return <div className={clsx('min-w-0 rounded-lg border border-line bg-surface', className)}>
    {title && <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
      <div><h2 className="font-semibold">{title}</h2>{subtitle && <p className="mt-0.5 text-xs text-fg-2 [overflow-wrap:anywhere]">{subtitle}</p>}</div>{actions}
    </div>}
    <div className={bodyClass ?? (padded ? 'p-4' : undefined)}>{children}</div>
  </div>
}

export function PageHeader({ title, description, subtitle, actions }: { title: ReactNode; description?: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
      <div className="min-w-0">
        <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.035em] text-fg sm:text-[30px]">{title}</h1>
        {(description ?? subtitle) && <div className="mt-1.5 text-[14px] text-fg-2">{description ?? subtitle}</div>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  )
}

export function Section({ id, title, description, actions, children, className }:
  { id?: string; title: ReactNode; description?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section id={id} className={clsx('scroll-mt-12', className)}>
      <div className="mb-3 flex flex-wrap items-end justify-between gap-x-4 gap-y-1">
        <div className="min-w-0">
          <h2 className="text-[16px] font-semibold tracking-[-0.015em] text-fg">{title}</h2>
          {description && <p className="mt-0.5 max-w-3xl text-[13.5px] text-fg-2">{description}</p>}
        </div>
        {actions && <div className="flex items-center gap-3 text-[13px]">{actions}</div>}
      </div>
      {children}
    </section>
  )
}

export function Spinner({ className }: { className?: string }) {
  return <Loader2 className={clsx('animate-spin text-fg-3', className ?? 'h-4 w-4')} />
}

export function Loading({ label = 'Loading' }: { label?: string }) {
  return <div className="flex items-center justify-center gap-2 py-24 text-[13px] text-fg-3" role="status"><Spinner /> {label}…</div>
}

export function Empty({ title, children, action, className, icon }: { title: string; children?: ReactNode; action?: ReactNode; className?: string; icon?: ReactNode }) {
  return (
    <div className={clsx('flex flex-col items-center px-6 py-10 text-center', className)}>
      {icon && <div className="mb-3 text-fg-3">{icon}</div>}
      <div className="text-[14px] font-medium text-fg">{title}</div>
      {children && <div className="mt-1 max-w-sm text-[13px] text-fg-2">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  )
}

export function ErrorNote({ error }: { error: unknown }) {
  const msg = error instanceof Error ? error.message : String(error)
  return (
    <div className="flex items-start gap-2 rounded-md border border-bad/30 bg-bad/5 px-3 py-2 text-[13px] text-fg" role="alert">
      <AlertCircle className="mt-[2px] h-4 w-4 shrink-0 text-bad" /> <span>{msg}</span>
    </div>
  )
}

export function Banner({ children, actions, tone = 'neutral' }: { children: ReactNode; actions?: ReactNode; tone?: 'neutral' | 'warn' }) {
  return (
    <div className={clsx('flex flex-wrap items-center justify-between gap-3 rounded-lg border px-4 py-3 text-[13.5px]',
      tone === 'warn' ? 'border-warn/30 bg-warn/5' : 'border-line bg-surface')}>
      <div className="min-w-0 flex-1">{children}</div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  )
}

/** A bordered strip of figures separated by hairlines (the 1px grid gap shows the border colour). */
export function Figures({ children, cols = 4, className }: { children: ReactNode; cols?: number; className?: string }) {
  return (
    <div style={{ '--cols': cols } as React.CSSProperties}
      className={clsx('grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-line bg-line md:[grid-template-columns:repeat(var(--cols),minmax(0,1fr))]',
        // An odd last figure spans the row on phones instead of leaving an empty cell.
        'max-md:[&>*:last-child:nth-child(odd)]:col-span-2', className)}>
      {children}
    </div>
  )
}

export function Figure({ label, value, detail, tone, hint }: { label: ReactNode; value: ReactNode; detail?: ReactNode; tone?: 'good' | 'bad' | 'warn'; hint?: string }) {
  return (
    <div className="min-w-0 bg-surface px-4 py-3.5" title={hint}>
      <div className="truncate text-[13px] text-fg-2">{label}</div>
      <div className={clsx('tnum mt-1.5 text-[26px] font-semibold leading-none tracking-[-0.03em]', tone === 'good' && 'text-good', tone === 'bad' && 'text-bad', tone === 'warn' && 'text-warn')}>{value}</div>
      {detail && <div className="mt-1.5 text-[12.5px] text-fg-3 sm:truncate">{detail}</div>}
    </div>
  )
}

const CONF: Record<Conf, [string, string]> = {
  insufficient: ['Too little data', 'bg-fg-3'],
  low: ['Low confidence', 'bg-warn'],
  moderate: ['Moderate confidence', 'bg-accent-ink'],
  high: ['High confidence', 'bg-good'],
}

export function Confidence({ c, n }: { c: Conf; n?: number }) {
  const [label, dot] = CONF[c]
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-[12px] text-fg-3" title="Based on how often this occurred; small samples can be noise.">
      <span className={clsx('h-1.5 w-1.5 rounded-full', dot)} />{label}{n !== undefined && <span className="tnum">· {n}</span>}
    </span>
  )
}

export function Badge({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={clsx('inline-flex items-center gap-1 whitespace-nowrap rounded-[5px] border border-line px-1.5 text-[12px] leading-5 text-fg-2', className)}>{children}</span>
}

export function Progress({ value, max, className }: { value: number; max: number; className?: string }) {
  const p = max > 0 ? Math.min(100, (100 * value) / max) : 0
  return (
    <div className={clsx('h-1 w-full overflow-hidden rounded-full bg-subtle', className)} role="progressbar" aria-valuenow={Math.round(p)} aria-valuemin={0} aria-valuemax={100}>
      <div className="h-full rounded-full bg-accent transition-[width] duration-500" style={{ width: `${p}%` }} />
    </div>
  )
}

export function Meter({ value, max = 100, tone = 'fg' }: { value: number; max?: number; tone?: 'fg' | 'accent' | 'bad' }) {
  return (
    <div className="h-1 w-full overflow-hidden rounded-full bg-subtle">
      <div className={clsx('h-full rounded-full', tone === 'fg' && 'bg-fg', tone === 'accent' && 'bg-accent', tone === 'bad' && 'bg-bad')}
        style={{ width: `${Math.max(0, Math.min(100, (100 * value) / (max || 1)))}%` }} />
    </div>
  )
}

export function Switch({ checked, onChange, label, description, disabled }:
  { checked: boolean; onChange: (v: boolean) => void; label: string; description?: ReactNode; disabled?: boolean }) {
  return (
    <div className={clsx('flex items-start justify-between gap-8 py-3.5', disabled && 'opacity-50')}>
      <div>
        <div className="text-[14px] font-medium">{label}</div>
        {description && <div className="mt-0.5 text-[13px] text-fg-2">{description}</div>}
      </div>
      <button type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}
        className={clsx('relative mt-0.5 h-5 w-9 shrink-0 rounded-full transition-colors after:absolute after:-inset-2.5 after:content-[""]', checked ? 'bg-accent' : 'bg-line-strong')}>
        <span className={clsx('absolute top-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-[left]', checked ? 'left-[18px]' : 'left-0.5')} />
      </button>
    </div>
  )
}

export function Tabs<T extends string>({ value, options, onChange, label }:
  { value: T; options: { value: T; label: ReactNode }[]; onChange: (v: T) => void; label?: string }) {
  return (
    <div role="tablist" aria-label={label} className="inline-flex h-10 max-w-full items-center overflow-x-auto rounded-md border border-line bg-subtle p-0.5 [scrollbar-width:none] sm:h-8">
      {options.map(o => (
        <button key={o.value} type="button" role="tab" aria-selected={value === o.value} onClick={() => onChange(o.value)}
          className={clsx('h-full whitespace-nowrap rounded-[5px] px-2.5 text-[13px] font-medium transition-colors',
            value === o.value ? 'bg-surface text-fg shadow-[0_0_0_1px_var(--line)]' : 'text-fg-2 hover:text-fg')}>
          {o.label}
        </button>
      ))}
    </div>
  )
}

export function Select({ value, onChange, children, className, label }:
  { value: string; onChange: (v: string) => void; children: ReactNode; className?: string; label: string }) {
  return (
    <select aria-label={label} value={value} onChange={e => onChange(e.target.value)}
      className={clsx('h-10 rounded-md border border-line bg-surface pl-2.5 pr-7 text-[13.5px] text-fg outline-none transition-colors hover:border-line-strong focus-visible:border-accent-ink sm:h-8', className)}>
      {children}
    </select>
  )
}

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input {...rest}
      className={clsx('h-10 rounded-md border border-line bg-surface px-2.5 text-[13.5px] text-fg outline-none transition-colors placeholder:text-fg-3 hover:border-line-strong focus:border-accent-ink sm:h-8', className)} />
  )
}

export function ResultMark({ outcome }: { outcome: 'win' | 'loss' | 'draw' | null }) {
  if (!outcome) return <span className="inline-block w-4" />
  const [l, cls] = { win: ['W', 'text-good'], loss: ['L', 'text-bad'], draw: ['D', 'text-fg-3'] }[outcome]
  return <span aria-label={outcome} className={clsx('inline-block w-4 text-[13px] font-semibold', cls)}>{l}</span>
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-line bg-subtle px-1 font-mono text-[11px] leading-[18px] text-fg-3">{children}</kbd>
}
