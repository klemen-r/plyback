import { expect, test, type Page } from '@playwright/test'
import type { Account } from '../src/lib/types'

const account: Account = {
  id: 1, username: 'state_fixture', display_name: 'State fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
  analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0,
}
const status = {
  data_revision: 1, accounts: 1, games: 0, analyzed: 0,
  engine: { name: null, error: null }, sync: { current_account: null },
  queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null },
}

async function mockApi(page: Page) {
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname
    if (path === '/api/auth/session') return route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    if (path === '/api/accounts') return route.fulfill({ json: [account] })
    if (path === '/api/status') return route.fulfill({ json: status })
    if (path === '/api/games') return route.fulfill({ json: { items: [], total: 0, page: 1, page_size: 50 } })
    if (path === '/api/games/openings') return route.fulfill({ json: [] })
    return route.fulfill({ status: 503, json: { detail: 'Not mocked' } })
  })
}

test('The Games search box follows the URL when the Games tab clears the search', async ({ page }) => {
  await mockApi(page)
  await page.goto('/games?q=smith')
  const box = page.getByRole('textbox', { name: 'Search games' })
  await expect(box).toHaveValue('smith')
  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Games', exact: true }).click()
  await expect(page).toHaveURL(/\/games$/)
  await expect(box).toHaveValue('')
  await box.fill('smith')
  await expect(page).toHaveURL(/\/games\?q=smith$/)
})

test('A pending Games search keeps a filter selected while typing', async ({ page }) => {
  await page.clock.install()
  await mockApi(page)
  await page.goto('/games')
  const search = page.getByRole('textbox', { name: 'Search games' })
  await expect(search).toBeVisible()
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 5000))
  await search.fill('smith')
  await page.getByLabel('Time control').selectOption('blitz')
  await expect(page).toHaveURL(/time_class=blitz/)

  await page.clock.runFor(300)
  await expect(page).toHaveURL(url => url.searchParams.get('q') === 'smith' && url.searchParams.get('time_class') === 'blitz')
  await expect(page.getByLabel('Time control')).toHaveValue('blitz')
})

test('The PGN file picker can be reached with the keyboard', async ({ page }) => {
  await mockApi(page)
  await page.goto('/import')
  const picker = page.locator('input[type=file]')
  await expect(picker).toBeAttached()
  expect(await picker.evaluate(el => getComputedStyle(el).display)).not.toBe('none')
  await picker.focus()
  await expect(picker).toBeFocused()
})
