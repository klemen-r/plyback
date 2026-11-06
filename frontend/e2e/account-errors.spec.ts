import { expect, test } from '@playwright/test'
import type { Account } from '../src/lib/types'

const fixture: Account = {
  id: 1, username: 'account_fixture', display_name: 'Account fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
  analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0,
}

test('Accounts reports mutation failures on the affected card and restores rejected switches', async ({ page }) => {
  let account = { ...fixture }
  let reject = true
  let removed = false
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname
    const method = route.request().method()
    if (path === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (path === '/api/accounts') {
      await route.fulfill({ json: [
        ...removed ? [] : [account],
        { ...fixture, id: 2, username: 'other_fixture', display_name: 'Other fixture' },
      ] })
    } else if (path === '/api/status') {
      await route.fulfill({ json: { data_revision: 1, accounts: 2, games: 0, analyzed: 0,
        engine: { name: null, error: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null } } })
    } else if (path.startsWith('/api/accounts/1')) {
      if (reject) await route.fulfill({ status: 503, json: { detail: `${method} database unavailable` } })
      else {
        if (method === 'PATCH') account = { ...account, ...route.request().postDataJSON() }
        if (method === 'DELETE') removed = true
        await route.fulfill({ json: method === 'PATCH' ? account : { requested: true, deleted: true, games_removed: 0 } })
      }
    } else throw new Error(`Unexpected request: ${method} ${path}`)
  })
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
