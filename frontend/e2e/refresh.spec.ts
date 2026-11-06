import { readFileSync } from 'node:fs'
import { expect, test, type Page, type Route } from '@playwright/test'

const insights = JSON.parse(readFileSync(new URL('../../rust/tests/fixtures/insights-expected.json', import.meta.url), 'utf8'))[0].result

async function mockPages(page: Page, handle: (route: Route, url: URL) => Promise<void>, revision: () => number) {
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (url.pathname === '/api/accounts') {
      await route.fulfill({ json: [{ id: 1, username: 'fixture', display_name: 'Fixture', ratings: {},
        games: 1, analyzed: 1, queued: 0, sync_status: 'idle' }] })
    } else if (url.pathname === '/api/status') {
      await route.fulfill({ json: { data_revision: revision(), accounts: 1, games: 1, analyzed: 1,
        engine: { name: 'Fixture', error: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null } } })
    } else if (url.pathname === '/api/games/openings') {
      await route.fulfill({ json: [] })
    } else await handle(route, url)
  })
  await page.clock.install()
}

test('Openings refreshes after a revision change even when account counts stay the same', async ({ page }) => {
  let revision = 1
  let reads = 0
  await mockPages(page, async route => {
    reads++
    await route.fulfill({ json: { color: 'white', plies: 16, total_games: 1, games: revision - 1,
      wins: revision - 1, draws: 0, losses: 0, nodes: [], positions: [], problems: [], merged: false, root: 0 } })
  }, () => revision)
  await page.goto('/openings')
  await expect(page.getByText('No reviewed games as White yet')).toBeVisible()
  revision = 2
  await page.clock.fastForward(16_000)
  await expect.poll(() => reads).toBe(2)
  await expect(page.getByText('No reviewed games as White yet')).toHaveCount(0)
})

test('Insights recovers from a transient error, stays quiet when unchanged, and refreshes on revision', async ({ page }) => {
  let revision = 1
  let reads = 0
  await mockPages(page, async route => {
    reads++
    await route.fulfill(reads === 1
      ? { status: 503, json: { detail: 'Temporary insight failure' } }
      : { json: insights })
  }, () => revision)
  await page.goto('/insights')
  await expect(page.getByText('Temporary insight failure')).toBeVisible()
  await page.clock.fastForward(16_000)
  await expect.poll(() => reads).toBe(2)
  await expect(page.getByText('Temporary insight failure')).toHaveCount(0)
  const poll = page.waitForResponse(r => r.url().endsWith('/api/status'))
  await page.clock.fastForward(60_000)
  await poll
  expect(reads).toBe(2)
  revision = 2
  await page.clock.fastForward(16_000)
  await expect.poll(() => reads).toBe(3)
})
