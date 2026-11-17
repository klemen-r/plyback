import { expect, test } from '@playwright/test'
import { account as makeAccount, status as makeStatus, mockApi } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'filters_fixture', display_name: 'Filters fixture' })], status: makeStatus(), handleFirst: true }

  test('Settings keeps Appearance and the access password when only the status request fails', async ({ page }) => {
    await mockApi(page, (route, url) => {
      const path = url.pathname
      if (path === '/api/status') { void route.fulfill({ status: 500, json: { detail: 'Status unavailable' } }); return true }
      if (path === '/api/settings') { void route.fulfill({ json: { analysis_depth: 16 } }); return true }
      return false
    }, undefined, apiOptions)
    await page.goto('/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(page.getByRole('alert').first()).toContainText('Status unavailable')
    await expect(page.getByText('Appearance', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Set access password' })).toBeVisible()
  })
})
