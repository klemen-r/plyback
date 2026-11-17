import { expect, test } from '@playwright/test'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'outage_fixture', display_name: 'Outage fixture' })], status: makeStatus(), emptyGames: true, handleFirst: true, fallbackStatus: 404 }

  test('A failed accounts request shows the error, not the first-run welcome', async ({ page }) => {
    await mockApi(page, async (route, url) => url.pathname === '/api/accounts' ? route.fulfill({ status: 500, json: { detail: 'Database unavailable' } }) : false, undefined, apiOptions)
    await page.goto('/')
    await expect(page.getByRole('alert')).toHaveText('Database unavailable')
    await expect(page.getByText('Every game you play, reviewed.')).toHaveCount(0)
    await expect(page.getByRole('link', { name: 'Connect account' })).toHaveCount(0)
    await page.goto('/reports')
    await expect(page.getByRole('alert')).toHaveText('Database unavailable')
    await expect(page.getByText('Connect a Chess.com account first')).toHaveCount(0)
  })

  test('The header says the server is not reachable instead of "Up to date"', async ({ page }) => {
    await mockApi(page, async (route, url) => url.pathname === '/api/status' ? route.abort('connectionrefused') : false, undefined, apiOptions)
    await page.goto('/')
    await expect(page.getByRole('link', { name: 'Server not reachable' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Up to date' })).toHaveCount(0)
  })

  test('A page that cannot load while the server is down loads once it is back', async ({ page }) => {
    let down = false
    await mockApi(page, () => false, undefined, apiOptions)
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
})
