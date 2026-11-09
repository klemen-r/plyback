/**
 * The "A − B" cell. Text and colour both come from the difference as displayed, so a gap that
 * rounds to zero reads as no difference rather than a green or red "+0.0".
 */
export function difference(a: number | null | undefined, b: number | null | undefined, digits: number, better: 'higher' | 'lower', unit = '') {
  if (a === null || a === undefined || b === null || b === undefined) return { text: '—', tone: 'none' as const }
  // `+ 0` turns a rounded -0 into 0.
  const d = Number((a - b).toFixed(digits)) + 0
  const text = `${d > 0 ? '+' : ''}${d.toFixed(digits)}${unit}`
  if (d === 0) return { text, tone: 'none' as const }
  return { text, tone: (better === 'higher') === d > 0 ? 'good' as const : 'bad' as const }
}

/** An account id from the address that is no longer connected is dropped, so the default applies. */
export function knownAccount(id: number | null, accounts: { id: number }[]): number | null {
  return id !== null && accounts.some(a => a.id === id) ? id : null
}
