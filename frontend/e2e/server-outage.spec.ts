import { expect, test, type Page } from '@playwright/test'
import type { Account } from '../src/lib/types'

const account: Account = {
  id: 1, username: 'outage_fixture', display_name: 'Outage fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
  analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0,
}
const status = {
  data_revision: 1, accounts: 1, games: 0, analyzed: 0,
  engine: { name: null, error: null }, sync: { current_account: null },
  queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null },
}

/** Mock the API; `fail` decides per path whether the request fails like a stopped server (abort) or with a 500. */
async function mockApi(page: Page, fail: (path: string) => 'abort' | 500 | null) {
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname
    const failure = fail(path)
    if (failure === 'abort') return route.abort('connectionrefused')
    if (failure === 500) return route.fulfill({ status: 500, json: { detail: 'Database unavailable' } })
    if (path === '/api/auth/session') return route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    if (path === '/api/accounts') return route.fulfill({ json: [account] })
    if (path === '/api/status') return route.fulfill({ json: status })
    if (path === '/api/games') return route.fulfill({ json: { items: [], total: 0, page: 1, page_size: 50 } })
    return route.fulfill({ status: 404, json: { detail: 'Not mocked' } })
  })
}

test('A failed accounts request shows the error, not the first-run welcome', async ({ page }) => {
  await mockApi(page, path => (path === '/api/accounts' ? 500 : null))
  await page.goto('/')
  await expect(page.getByRole('alert')).toHaveText('Database unavailable')
  await expect(page.getByText('Every game you play, reviewed.')).toHaveCount(0)
  await expect(page.getByRole('link', { name: 'Connect account' })).toHaveCount(0)
  await page.goto('/reports')
  await expect(page.getByRole('alert')).toHaveText('Database unavailable')
  await expect(page.getByText('Connect a Chess.com account first')).toHaveCount(0)
})

test('The header says the server is not reachable instead of "Up to date"', async ({ page }) => {
  await mockApi(page, path => (path === '/api/status' ? 'abort' : null))
  await page.goto('/')
  await expect(page.getByRole('link', { name: 'Server not reachable' })).toBeVisible()
  await expect(page.getByRole('link', { name: 'Up to date' })).toHaveCount(0)
})

test('A page that cannot load while the server is down loads once it is back', async ({ page }) => {
  let down = false
  await mockApi(page, () => null)
  await page.route(/\/assets\/Games-[^/]*\.js$/, route => (down ? route.abort('connectionrefused') : route.continue()))
  // While "down", page loads fail too, so an early reload would land on the browser's error page.
  await page.route(url => !url.pathname.startsWith('/api/') && !url.pathname.startsWith('/assets/'),
    route => (down ? route.abort('connectionrefused') : route.continue()))
  await page.goto('/')
  await expect(page.getByRole('link', { name: 'Up to date' })).toBeVisible()
  down = true
  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Games', exact: true }).click()
  await page.waitForTimeout(4000)
  expect(page.url()).toMatch(/\/games$/)
  await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible()
  down = false
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible({ timeout: 15_000 })
})
