import clsx from 'clsx'

/** The Plyback mark: three squares in a knight's L, the landing square in the cover colour. */
export function Mark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} className={clsx('shrink-0', className)} aria-hidden="true">
      <rect x="10" y="4" width="17" height="17" rx="2.5" fill="currentColor" />
      <rect x="10" y="24" width="17" height="17" rx="2.5" fill="currentColor" opacity="0.45" />
      <rect x="10" y="44" width="17" height="17" rx="2.5" fill="currentColor" opacity="0.2" />
      <rect x="31" y="4" width="17" height="17" rx="2.5" fill="var(--accent-ink)" />
    </svg>
  )
}
