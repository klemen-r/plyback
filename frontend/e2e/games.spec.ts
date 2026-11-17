import { expect, test } from '@playwright/test'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'large', display_name: 'Large', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 300, analyzed: 300 }), makeAccount({ id: 2, username: 'small', display_name: 'Small', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ accounts: 2, games: 302, analyzed: 302, engine: { name: 'Fixture', error: null, path: null } }), emptyOpenings: true }

  test('Games: another account starts on its first page, and a page past the end moves back', async ({ page }) => {
    const requests: URL[] = []
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname !== '/api/games') return route.fulfill({ json: {} })
      requests.push(url)
      const total = url.searchParams.get('account_id') === '2' ? 2 : 300
      await route.fulfill({ json: { total, page: Number(url.searchParams.get('page') ?? 1), page_size: 30, items: [] } })
    }, undefined, apiOptions)
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

  test('Games: an invalid page in the address falls back to the first page', async ({ page }) => {
    const requests: URL[] = []
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname !== '/api/games') return route.fulfill({ json: {} })
      requests.push(url)
      await route.fulfill({ json: { total: 300, page: 1, page_size: 30, items: [] } })
    }, undefined, apiOptions)
    for (const bad of ['abc', '0', '-2', '2.5']) {
      await page.goto(`/games?page=${bad}`)
      await expect(page).not.toHaveURL(/page=/)
      await expect(page.getByText('Page 1 of 10')).toBeVisible()
    }
    expect(requests.every(u => (u.searchParams.get('page') ?? '1') === '1')).toBe(true)
    expect(errors).toEqual([])
  })
})

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'state_fixture', display_name: 'State fixture' })], status: makeStatus(), emptyGames: true, emptyOpenings: true, handleFirst: true }

  test('The Games search box follows the URL when the Games tab clears the search', async ({ page }) => {
    await mockApi(page, undefined, undefined, apiOptions)
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
    await mockApi(page, undefined, undefined, apiOptions)
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
})
