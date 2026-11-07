import { readFileSync } from 'node:fs'
import { expect, test, type Page, type Route } from '@playwright/test'

// Fixes for the 2026-11-06 bug hunt. All API data is synthetic.
const insights = JSON.parse(readFileSync(new URL('../../rust/tests/fixtures/insights-expected.json', import.meta.url), 'utf8'))[0].result

const account = (id: number, name: string, games: number) => ({
  id, username: name.toLowerCase(), display_name: name, avatar: null, title: null, country: null, profile_url: '',
  joined: null, ratings: {}, added_at: 1, last_sync_at: 1, sync_status: 'idle', sync_error: null, sync_enabled: false,
  auto_analyze: false, analyze_history: false, import_since: null, games, analyzed: games, queued: 0,
})

async function fixture(page: Page, handle: (route: Route, url: URL) => Promise<void>) {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (url.pathname === '/api/accounts') {
      await route.fulfill({ json: [account(1, 'Large', 300), account(2, 'Small', 2)] })
    } else if (url.pathname === '/api/status') {
      await route.fulfill({ json: { data_revision: 1, accounts: 2, games: 302, analyzed: 302,
        engine: { name: 'Fixture', error: null, path: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null } } })
    } else if (url.pathname === '/api/games/openings') {
      await route.fulfill({ json: [] })
    } else await handle(route, url)
  })
  return errors
}

test('Games: another account starts on its first page, and a page past the end moves back', async ({ page }) => {
  const requests: URL[] = []
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname !== '/api/games') return route.fulfill({ json: {} })
    requests.push(url)
    const total = url.searchParams.get('account_id') === '2' ? 2 : 300
    await route.fulfill({ json: { total, page: Number(url.searchParams.get('page') ?? 1), page_size: 30, items: [] } })
  })
  await page.goto('/games?page=8')
  await expect(page.getByText('Page 8 of 10')).toBeVisible()
  await page.getByRole('button', { name: 'Switch account' }).click()
  await page.getByRole('menuitem', { name: 'Small' }).click()
  await expect(page).not.toHaveURL(/page=/)
  await expect.poll(() => requests.at(-1)?.searchParams.get('account_id')).toBe('2')
  expect(requests.at(-1)?.searchParams.get('page')).toBe('1')
  // A link to a page that no longer exists lands on the last one.
  await page.getByRole('button', { name: 'Switch account' }).click()
  await page.getByRole('menuitem', { name: 'Large' }).click()
  await page.goto('/games?page=50')
  await expect(page).toHaveURL(/page=10/)
  await expect(page.getByText('Page 10 of 10')).toBeVisible()
  expect(errors).toEqual([])
})

test('Insights: the rating chart follows a time-control filter that removes its current choice', async ({ page }) => {
  const points = (n: number, start: number) => Array.from({ length: n }, (_, i) => ({ t: 1_790_000_000 + i * 3600, rating: start + i, game_id: i + 1 }))
  const rating = (n: number, start: number) => ({ points: points(n, start), start, end: start + n - 1, change: n - 1, peak: start + n - 1, low: start, games: n })
  const errors = await fixture(page, async (route, url) => {
    if (!url.pathname.endsWith('/insights')) return route.fulfill({ json: {} })
    const bulletOnly = url.searchParams.getAll('time_classes').join(',').includes('bullet')
    await route.fulfill({ json: { ...insights, ratings: bulletOnly ? { bullet: rating(4, 700) } : { blitz: rating(6, 500), bullet: rating(4, 700) } } })
  })
  await page.goto('/insights')
  await expect(page.getByText('500 → 505 (+5) · peak 505')).toBeVisible()
  await page.getByRole('group', { name: 'Time control' }).getByRole('button', { name: 'bullet' }).click()
  await expect(page.getByText('700 → 703 (+3) · peak 703')).toBeVisible()
  await expect(page.getByText('No rated games in this selection')).toHaveCount(0)
  expect(errors).toEqual([])
})

test('Games: an invalid page in the address falls back to the first page', async ({ page }) => {
  const requests: URL[] = []
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname !== '/api/games') return route.fulfill({ json: {} })
    requests.push(url)
    await route.fulfill({ json: { total: 300, page: 1, page_size: 30, items: [] } })
  })
  for (const bad of ['abc', '0', '-2', '2.5']) {
    await page.goto(`/games?page=${bad}`)
    await expect(page).not.toHaveURL(/page=/)
    await expect(page.getByText('Page 1 of 10')).toBeVisible()
  }
  expect(requests.every(u => (u.searchParams.get('page') ?? '1') === '1')).toBe(true)
  expect(errors).toEqual([])
})

test('Import keeps a large file out of the text box and warns about too many games', async ({ page }) => {
  let posted = 0
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/games/import') { posted++; return route.fulfill({ json: { imported: 2, existing: 0, game_ids: [1, 2], errors: [] } }) }
    await route.fulfill({ json: {} })
  })
  const game = (i: number) => `[Event "Fixture ${i}"]\n[White "A"]\n[Black "B"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *\n\n`
  await page.goto('/import')
  const input = page.locator('input[type=file]')
  await input.setInputFiles({ name: 'big.pgn', mimeType: 'application/x-chess-pgn', buffer: Buffer.from(Array.from({ length: 3000 }, (_, i) => game(i)).join('')) })
  await expect(page.getByText(/big\.pgn · 0\.\d MB · about 3,000 games/)).toBeVisible()
  await expect(page.getByLabel('PGN text')).toHaveCount(0)
  await page.getByRole('button', { name: 'Import' }).click()
  await expect(page.getByText(/2.*new game\(s\) imported/)).toBeVisible()
  expect(posted).toBe(1)
  await input.setInputFiles({ name: 'huge.pgn', mimeType: 'application/x-chess-pgn', buffer: Buffer.from(Array.from({ length: 10_001 }, (_, i) => `[Event "${i}"]\n1. e4 *\n`).join('')) })
  await expect(page.getByText(/about 10,001 games\. Import at most 10,000/)).toBeVisible()
  await expect(page.getByRole('button', { name: 'Import' })).toBeDisabled()
  expect(errors).toEqual([])
})
