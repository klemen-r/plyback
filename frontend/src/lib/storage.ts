/** Storage can be unavailable in private browsing or when the browser denies access. */
export function readStorage(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}

export function writeStorage(key: string, value: string): void {
  try { localStorage.setItem(key, value) } catch { /* Keep the current in-memory preference. */ }
}

export function storedChoice<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  const value = readStorage(key)
  return allowed.includes(value as T) ? value as T : fallback
}
