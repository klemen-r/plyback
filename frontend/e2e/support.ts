import { expect, type Page, type Route } from '@playwright/test'
import type { Account } from '../src/lib/types'

export function account(overrides: Partial<Account> = {}): Account {
  return {
    id: 1, username: 'fixture', display_name: 'Fixture', avatar: null, title: null,
    country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
    sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
    analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0,
    ...overrides,
  }
}

type MockStatus = {
  data_revision?: number
  accounts: number
  games: number
  analyzed: number
  engine: { name: string | null; error: string | null; path?: string | null }
  sync: { current_account: number | null }
  queue: { queued: number; running: number; failed: number; paused: boolean; eta_seconds: number | null }
}

export function status(overrides: Partial<MockStatus> = {}): MockStatus {
  return {
    data_revision: 1, accounts: 1, games: 0, analyzed: 0,
    engine: { name: null, error: null }, sync: { current_account: null },
    queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null },
    ...overrides,
  }
}

type ApiOptions = {
  accounts?: unknown[]
  status?: MockStatus
  handleFirst?: boolean
  emptyOpenings?: boolean
  emptyGames?: boolean
  fallbackStatus?: number
}
type ApiHandler = (route: Route, url: URL) => Promise<unknown> | unknown

/** All API data is synthetic; the isolated server only supplies the built interface.
 * A handler used before defaults returns false to defer to those defaults.
 */
export async function mockApi(page: Page, handle: ApiHandler = () => false, revision?: () => number, options: ApiOptions = {}) {
  const errors: string[] = []
  const seen: URL[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    seen.push(url)
    if (options.handleFirst && await handle(route, url) !== false) return
    if (url.pathname === '/api/auth/session') return route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    if (url.pathname === '/api/accounts') return route.fulfill({ json: options.accounts ?? [account()] })
    if (url.pathname === '/api/status') return route.fulfill({ json: { ...(options.status ?? status()), ...revision ? { data_revision: revision() } : {} } })
    if (options.emptyGames && url.pathname === '/api/games') return route.fulfill({ json: { items: [], total: 0, page: 1, page_size: 50 } })
    if (options.emptyOpenings && url.pathname === '/api/games/openings') return route.fulfill({ json: [] })
    if (!options.handleFirst) await handle(route, url)
    else await route.fulfill({ status: options.fallbackStatus ?? 503, json: { detail: 'Not mocked' } })
  })
  return { errors, seen }
}

export async function playMove(page: Page, from: string, to: string, orientation: 'white' | 'black' = 'white') {
  // The board is rebuilt when it becomes playable, so wait for the playable one before selecting
  // squares. Locator clicks also check stability and use its current position if the page scrolls.
  const board = page.locator('.cg-wrap.manipulable cg-board')
  await expect(board).toBeVisible()
  // Keep the board clear of the sticky header before calculating square coordinates.
  await board.evaluate(el => el.scrollIntoView({ block: 'center' }))
  const size = await board.boundingBox()
  if (!size) throw new Error('Chessboard is not visible')
  const at = (square: string) => {
    const file = square.charCodeAt(0) - 97
    const rank = Number(square[1]) - 1
    return {
      x: ((orientation === 'white' ? file : 7 - file) + 0.5) * size.width / 8,
      y: ((orientation === 'white' ? 7 - rank : rank) + 0.5) * size.height / 8,
    }
  }
  await board.click({ position: at(from) })
  await expect(board.locator('square.selected')).toBeVisible()
  await board.click({ position: at(to) })
}
