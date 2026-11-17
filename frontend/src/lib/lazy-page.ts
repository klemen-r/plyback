import { lazy, type ComponentType } from 'react'

const RELOAD_KEY = 'plyback.chunk-reload'
const RETRY_MS = 3000

async function serverAnswers() {
  try {
    await fetch('/', { method: 'HEAD', cache: 'no-store' })
    return true
  } catch {
    return false
  }
}

/**
 * Each page is a separate chunk. A chunk fails to load when the server is briefly down, or when
 * this tab was opened on an earlier build whose chunks no longer exist. In both cases, wait until
 * the server answers and then reload, so the tab never ends up on the browser's own error page.
 * The shell stays up meanwhile and the header shows that the server cannot be reached.
 * A chunk that is still missing straight after a reload is a real error and is rethrown.
 */
export function lazyPage<T extends ComponentType<any>>(load: () => Promise<{ default: T }>) {
  return lazy(async () => {
    try {
      return await load()
    } catch (error) {
      let last: number
      try { last = Number(sessionStorage.getItem(RELOAD_KEY)) || 0 } catch { throw error }
      if (Date.now() - last < 10_000) throw error
      while (!(await serverAnswers())) await new Promise(resolve => setTimeout(resolve, RETRY_MS))
      try { sessionStorage.setItem(RELOAD_KEY, String(Date.now())) } catch { /* storage unavailable */ }
      location.reload()
      return new Promise<never>(() => {})
    }
  })
}
