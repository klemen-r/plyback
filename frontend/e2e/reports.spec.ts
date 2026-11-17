import { expect, test, type Route } from '@playwright/test'
import { readFileSync } from 'node:fs'
import type { Report } from '../src/lib/types'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  test.use({ actionTimeout: 20_000 })
  const apiOptions = { accounts: [makeAccount({ username: 'review_fixture', display_name: 'Review fixture', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 1 })], status: makeStatus({ games: 1, engine: { name: 'Fixture', error: null, path: null } }) }

  test('deleting one report does not spin or close another row’s confirmation', async ({ page }) => {
    const report = (id: number): Report => ({ id, account_id: 1, title: `Fixture report ${id}`, status: 'ready', total_games: 3, analyzed_games: 3,
      created_at: 1, filters: {} } as unknown as Report)
    let pending: Route | undefined
    let reports = [report(1), report(2)]
    const { errors } = await mockApi(page, async (route, url) => {
      if (route.request().method() === 'DELETE') { pending = route; return }
      if (url.pathname === '/api/accounts/1/reports') await route.fulfill({ json: reports })
      else await route.fulfill({ json: [] })
    }, undefined, apiOptions)
    await page.goto('/reports')
    const row = (id: number) => page.locator('div.flex.items-center.gap-4').filter({ hasText: `Fixture report ${id}` })
    await row(1).getByRole('button', { name: 'Delete report' }).click()
    await row(1).getByRole('button', { name: 'Delete', exact: true }).click()
    await expect.poll(() => !!pending).toBe(true)
    await row(2).getByRole('button', { name: 'Delete report' }).click()
    await expect(row(2).getByRole('button', { name: 'Delete', exact: true })).toBeEnabled()
    await expect(row(1).getByRole('button', { name: 'Delete', exact: true })).toBeDisabled()
    reports = [report(2)]
    await pending!.fulfill({ json: { deleted: true } })
    await expect(row(1)).toHaveCount(0)
    // Report 2's confirmation stays open after report 1 is gone.
    await expect(row(2).getByRole('button', { name: 'Delete', exact: true })).toBeEnabled()
    expect(errors).toEqual([])
  })
})

test.describe(() => {
  const insights = JSON.parse(readFileSync(new URL('../../rust/tests/fixtures/insights-expected.json', import.meta.url), 'utf8'))[0].result
  const apiOptions = { accounts: [makeAccount({ username: 'large', display_name: 'Large', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 300, analyzed: 300 }), makeAccount({ id: 2, username: 'small', display_name: 'Small', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ accounts: 2, games: 302, analyzed: 302, engine: { name: 'Fixture', error: null, path: null } }), emptyOpenings: true }

  test('A saved report displays the review count from its snapshot', async ({ page }) => {
    const { games, analyzed } = insights.data_quality
    await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/reports/7') {
        return route.fulfill({ json: {
          id: 7, account_id: 1, title: 'Fixture snapshot', status: 'ready', completed_at: 1790000000,
          total_games: games, analyzed_games: 0, report: insights,
        } })
      }
      await route.fulfill({ json: {} })
    }, undefined, apiOptions)
    await page.goto('/reports/7')
    await expect(page.getByText(/Snapshot from/)).toContainText(`${analyzed} of ${games} games analysed`, { timeout: 1500 })
  })
})
