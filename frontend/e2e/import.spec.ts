import { expect, test } from '@playwright/test'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'large', display_name: 'Large', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 300, analyzed: 300 }), makeAccount({ id: 2, username: 'small', display_name: 'Small', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ accounts: 2, games: 302, analyzed: 302, engine: { name: 'Fixture', error: null, path: null } }), emptyOpenings: true }

  test('Import keeps a large file out of the text box and notes the game limit', async ({ page }) => {
    let posted = 0
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/games/import') { posted++; return route.fulfill({ json: { imported: 2, existing: 0, game_ids: [1, 2], errors: [] } }) }
      await route.fulfill({ json: {} })
    }, undefined, apiOptions)
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
    // The count is an estimate (tags in comments or a broken tail also match), so the server decides.
    await expect(page.getByText(/about 10,001 games \(at most 10,000 are imported at once\)/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Import' })).toBeEnabled()
    expect(errors).toEqual([])
  })

  test('Import waits for the newest file and ignores an older read', async ({ page }) => {
    const posted: string[] = []
    await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/games/import') {
        posted.push(route.request().postDataJSON().pgn)
        return route.fulfill({ json: { imported: 1, existing: 0, game_ids: [1], errors: [] } })
      }
      await route.fulfill({ json: {} })
    }, undefined, apiOptions)
    await page.addInitScript(() => {
      File.prototype.text = function () {
        return new Promise<string>(resolve => {
          window.addEventListener(`read-${this.name}`, () => resolve(`[Event "${this.name}"]\n1. e4 *`), { once: true })
        })
      }
    })
    await page.goto('/import')
    const input = page.locator('input[type=file]')
    const text = page.getByRole('textbox', { name: 'PGN text' })
    const submit = page.getByRole('button', { name: 'Import', exact: true })
    await text.fill('[Event "Previous paste"]\n1. d4 *')
    for (const name of ['older.pgn', 'latest.pgn']) {
      await input.setInputFiles({ name, mimeType: 'application/x-chess-pgn', buffer: Buffer.from('1. e4 *') })
      await expect(submit).toBeDisabled({ timeout: 1500 })
    }
    const newest = '[Event "latest.pgn"]\n1. e4 *'
    await page.evaluate(() => { window.dispatchEvent(new Event('read-latest.pgn')) })
    await expect(text).toHaveValue(newest)
    await page.evaluate(() => { window.dispatchEvent(new Event('read-older.pgn')) })
    await submit.click()
    await expect(page.getByText(/1.*new game\(s\) imported/)).toBeVisible()
    expect(posted).toEqual([newest])
  })
})

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'state_fixture', display_name: 'State fixture' })], status: makeStatus(), emptyGames: true, emptyOpenings: true, handleFirst: true }

  test('The PGN file picker can be reached with the keyboard', async ({ page }) => {
    await mockApi(page, undefined, undefined, apiOptions)
    await page.goto('/import')
    const picker = page.locator('input[type=file]')
    await expect(picker).toBeAttached()
    expect(await picker.evaluate(el => getComputedStyle(el).display)).not.toBe('none')
    await picker.focus()
    await expect(picker).toBeFocused()
  })
})
