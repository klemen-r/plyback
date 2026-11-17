import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  const insights = JSON.parse(readFileSync(new URL('../../rust/tests/fixtures/insights-expected.json', import.meta.url), 'utf8'))[0].result
  const apiOptions = { accounts: [makeAccount({ username: 'large', display_name: 'Large', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 300, analyzed: 300 }), makeAccount({ id: 2, username: 'small', display_name: 'Small', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ accounts: 2, games: 302, analyzed: 302, engine: { name: 'Fixture', error: null, path: null } }), emptyOpenings: true }

  test('Insights: the rating chart follows a time-control filter that removes its current choice', async ({ page }) => {
    const points = (n: number, start: number) => Array.from({ length: n }, (_, i) => ({ t: 1_790_000_000 + i * 3600, rating: start + i, game_id: i + 1 }))
    const rating = (n: number, start: number) => ({ points: points(n, start), start, end: start + n - 1, change: n - 1, peak: start + n - 1, low: start, games: n })
    const { errors } = await mockApi(page, async (route, url) => {
      if (!url.pathname.endsWith('/insights')) return route.fulfill({ json: {} })
      const bulletOnly = url.searchParams.getAll('time_classes').join(',').includes('bullet')
      await route.fulfill({ json: { ...insights, ratings: bulletOnly ? { bullet: rating(4, 700) } : { blitz: rating(6, 500), bullet: rating(4, 700) } } })
    }, undefined, apiOptions)
    await page.goto('/insights')
    await expect(page.getByText('500 → 505 (+5) · peak 505')).toBeVisible()
    await page.getByRole('group', { name: 'Time control' }).getByRole('button', { name: 'bullet' }).click()
    await expect(page.getByText('700 → 703 (+3) · peak 703')).toBeVisible()
    await expect(page.getByText('No rated games in this selection')).toHaveCount(0)
    expect(errors).toEqual([])
  })
})

test.describe(() => {
  const insights = JSON.parse(readFileSync(new URL('../../rust/tests/fixtures/insights-expected.json', import.meta.url), 'utf8'))[0].result
  const apiOptions = { accounts: [{ id: 1, username: 'fixture', display_name: 'Fixture', ratings: {}, games: 1, analyzed: 1, queued: 0, sync_status: 'idle' }], status: makeStatus({ games: 1, analyzed: 1, engine: { name: 'Fixture', error: null } }), emptyOpenings: true }

  test('Insights recovers from a transient error, stays quiet when unchanged, and refreshes on revision', async ({ page }) => {
    let revision = 1
    let reads = 0
    await mockApi(page, async route => {
      reads++
      await route.fulfill(reads === 1
        ? { status: 503, json: { detail: 'Temporary insight failure' } }
        : { json: insights })
    }, () => revision, apiOptions)
    await page.clock.install()
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
})

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'filters_fixture', display_name: 'Filters fixture' })], status: makeStatus(), handleFirst: true }

  test('Insights shows a stored opening filter the account has no games in, and can clear it', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ci.insightFilters', JSON.stringify({ opening: 'Nonexistent Opening' })))
    const { seen } = await mockApi(page, (route, url) => {
      const path = url.pathname
      if (path === '/api/games/openings') { void route.fulfill({ json: [{ family: 'Italian Game', n: 3 }] }); return true }
      return false
    }, undefined, apiOptions)
    await page.goto('/insights')
    const select = page.getByRole('combobox', { name: 'Opening' })
    await expect(select).toHaveValue('Nonexistent Opening')
    await select.selectOption('')
    await expect.poll(() => seen.some(u => u.pathname.endsWith('/insights') && !u.searchParams.has('opening'))).toBe(true)
  })
})
