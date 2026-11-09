import { expect, test, type Page } from '@playwright/test'
import type { Account } from '../src/lib/types'

const account: Account = {
  id: 1, username: 'filters_fixture', display_name: 'Filters fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
  analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0,
}
const status = {
  data_revision: 1, accounts: 1, games: 0, analyzed: 0,
  engine: { name: null, error: null }, sync: { current_account: null },
  queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null },
}

/** Mock the API; `answer` may handle a path first and returns true when it did. Every request URL is recorded. */
async function mockApi(page: Page, answer: (path: string, route: Parameters<Parameters<Page['route']>[1]>[0]) => Promise<boolean> | boolean = () => false) {
  const seen: URL[] = []
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    seen.push(url)
    if (await answer(url.pathname, route)) return
    if (url.pathname === '/api/auth/session') return route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    if (url.pathname === '/api/accounts') return route.fulfill({ json: [account] })
    if (url.pathname === '/api/status') return route.fulfill({ json: status })
    return route.fulfill({ status: 503, json: { detail: 'Not mocked' } })
  })
  return seen
}

test('Settings keeps Appearance and the access password when only the status request fails', async ({ page }) => {
  await mockApi(page, (path, route) => {
    if (path === '/api/status') { void route.fulfill({ status: 500, json: { detail: 'Status unavailable' } }); return true }
    if (path === '/api/settings') { void route.fulfill({ json: { analysis_depth: 16 } }); return true }
    return false
  })
  await page.goto('/settings')
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
  await expect(page.getByRole('alert').first()).toContainText('Status unavailable')
  await expect(page.getByText('Appearance', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Set access password' })).toBeVisible()
})

test('Openings treats an unknown time control in the URL as all time controls', async ({ page }) => {
  const seen = await mockApi(page)
  await page.goto('/openings?tc=bogus')
  await expect(page.getByRole('combobox', { name: 'Time control' })).toHaveValue('all')
  await expect.poll(() => seen.some(u => u.pathname.endsWith('/repertoire'))).toBe(true)
  expect(seen.filter(u => u.pathname.endsWith('/repertoire')).every(u => !u.searchParams.has('time_class'))).toBe(true)
})

test('Insights shows a stored opening filter the account has no games in, and can clear it', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('ci.insightFilters', JSON.stringify({ opening: 'Nonexistent Opening' })))
  const seen = await mockApi(page, (path, route) => {
    if (path === '/api/games/openings') { void route.fulfill({ json: [{ family: 'Italian Game', n: 3 }] }); return true }
    return false
  })
  await page.goto('/insights')
  const select = page.getByRole('combobox', { name: 'Opening' })
  await expect(select).toHaveValue('Nonexistent Opening')
  await select.selectOption('')
  await expect.poll(() => seen.some(u => u.pathname.endsWith('/insights') && !u.searchParams.has('opening'))).toBe(true)
})
