import { expect, test } from '@playwright/test'
import { account as makeAccount, status as makeStatus, mockApi, waitForAnalysis } from './support'

const fixture = makeAccount({ username: 'account_fixture', display_name: 'Account fixture' })

test('Accounts reports mutation failures on the affected card and restores rejected switches', async ({ page }) => {
  let account = { ...fixture }
  let reject = true
  let removed = false
  await mockApi(page, async (route, url) => {
    const path = url.pathname
    const method = route.request().method()
    if (path === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (path === '/api/accounts') {
      await route.fulfill({ json: [
        ...removed ? [] : [account],
        { ...fixture, id: 2, username: 'other_fixture', display_name: 'Other fixture' },
      ] })
    } else if (path === '/api/status') {
      await route.fulfill({ json: makeStatus({ accounts: 2 }) })
    } else if (path.startsWith('/api/accounts/1')) {
      if (reject) await route.fulfill({ status: 503, json: { detail: `${method} database unavailable` } })
      else {
        if (method === 'PATCH') account = { ...account, ...route.request().postDataJSON() }
        if (method === 'DELETE') removed = true
        await route.fulfill({ json: method === 'PATCH' ? account : { requested: true, deleted: true, games_removed: 0 } })
      }
    } else throw new Error(`Unexpected request: ${method} ${path}`)
  }, undefined, { handleFirst: true })
  await page.goto('/accounts')
  const cards = page.locator('main .space-y-4 > div')
  const card = cards.filter({ hasText: 'Account fixture' })
  const other = cards.filter({ hasText: 'Other fixture' })

  for (const name of ['Sync automatically', 'Review new games', 'Review history']) {
    reject = true
    const toggle = card.getByRole('switch', { name, exact: true })
    await expect(toggle).toHaveAttribute('aria-checked', 'true')
    await toggle.click()
    await expect(card.getByRole('alert')).toHaveText('PATCH database unavailable')
    await expect(toggle).toHaveAttribute('aria-checked', 'true')
    await expect(other.getByRole('alert')).toHaveCount(0)
    reject = false
    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'false')
    await expect(card.getByRole('alert')).toHaveCount(0)
  }

  reject = true
  await card.getByRole('button', { name: 'Sync', exact: true }).click()
  await expect(card.getByRole('alert')).toHaveText('POST database unavailable')
  await expect(other.getByRole('alert')).toHaveCount(0)
  reject = false
  await card.getByRole('button', { name: 'Sync', exact: true }).click()
  await expect(card.getByRole('alert')).toHaveCount(0)

  reject = true
  await card.getByRole('button', { name: 'Remove…', exact: true }).click()
  await card.getByRole('button', { name: 'Remove', exact: true }).click()
  await expect(card.getByRole('alert')).toHaveText('DELETE database unavailable')
  await expect(card).toBeVisible()
  await expect(other.getByRole('alert')).toHaveCount(0)
  reject = false
  await card.getByRole('button', { name: 'Remove', exact: true }).click()
  await expect(card).toHaveCount(0)
  await expect(other).toBeVisible()
})

test('connect a Lichess account, import its game and open an engine review', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  try {
    await page.goto('/accounts')
    await page.getByRole('tab', { name: 'Lichess', exact: true }).click()
    await expect(page.getByRole('tab', { name: 'Lichess', exact: true })).toHaveAttribute('aria-selected', 'true')
    await page.screenshot({ path: test.info().outputPath('lichess-accounts.png') })
    await page.getByLabel('Lichess username').fill('no_such_player')
    await page.getByRole('button', { name: 'Connect', exact: true }).click()
    await expect(page.getByText(/No Lichess player named/)).toBeVisible()
    await page.getByLabel('Lichess username').fill('lichess_fixture')
    await page.getByRole('button', { name: 'Connect', exact: true }).click()
    await waitForAnalysis(page, 1, { platform: 'lichess', username: 'lichess_fixture' })
    await page.getByRole('button', { name: 'Switch account' }).click()
    await page.getByRole('menuitem').filter({ hasText: 'lichess_fixture' }).click()
    await page.goto('/games')
    await expect(page.getByText('1 game played by lichess_fixture')).toBeVisible()
    const game = page.getByRole('row').filter({ hasText: 'FixtureFriend' })
    await expect(game.getByRole('img', { name: 'Lichess', exact: true })).toBeVisible()
    await game.press('Enter')
    await expect(page.locator('cg-board')).toBeVisible()
    await expect(page.getByRole('img', { name: 'Evaluation graph' })).toBeVisible()
    const originalGame = page.getByRole('link', { name: 'View on Lichess', exact: true })
    const analysisBoard = page.getByRole('link', { name: 'Analyse on Lichess', exact: true })
    await expect(originalGame).toHaveText('View on Lichess')
    await expect(originalGame).toHaveAttribute('href', 'https://lichess.org/fixture1')
    await expect(analysisBoard).toHaveText('Analyse on Lichess')
    await expect(analysisBoard).toHaveAttribute('href', 'https://lichess.org/analysis/rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR_w_KQkq_-_0_1')
    await expect(page.getByRole('link', { name: 'Lichess', exact: true })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Move quality', exact: true })).toBeVisible()
    expect(errors).toEqual([])
  } finally {
    // Only this test's account is removed; the server uses its isolated temporary database.
    const accounts = await (await page.request.get('/api/accounts')).json() as { id: number; platform: string; username: string }[]
    const connected = accounts.find(a => a.platform === 'lichess' && a.username === 'lichess_fixture')
    if (connected) {
      const response = await page.request.delete(`/api/accounts/${connected.id}`, {
        headers: { 'X-Requested-With': 'plyback', Origin: new URL(page.url()).origin },
      })
      expect(response.ok(), await response.text()).toBe(true)
    }
  }
})
