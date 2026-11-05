// Invoked by scripts/benchmark_api.py against its disposable, background-free server.
// Every route gets a fresh browser context so no cached asset hides transfer size.
import { chromium } from '@playwright/test'

const [origin, game] = process.argv.slice(2)
const url = new URL(origin)
if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1') throw new Error('Expected the local benchmark server')
if (!/^\d+$/.test(game)) throw new Error('Expected a fixture game ID')
const routes = { overview: '/', games: '/games', review: `/games/${game}`, insights: '/insights',
  practice: '/practice', openings: '/openings', compare: '/compare', reports: '/reports',
  accounts: '/accounts', activity: '/activity', settings: '/settings', methodology: '/methodology', import: '/import' }
const browser = await chromium.launch()
const result = {}
try {
  for (const [name, path] of Object.entries(routes)) {
    const context = await browser.newContext()
    try {
      const page = await context.newPage()
      const errors = []
      page.on('pageerror', error => errors.push(error.message))
      const response = await page.goto(new URL(path, url).href)
      if (!response?.ok()) throw new Error(`Page failed: ${name}`)
      await page.waitForLoadState('networkidle')
      await page.evaluate(() => document.fonts.ready)
      if (errors.length) throw new Error(`Browser error on ${name}: ${errors.join('; ')}`)
      result[name] = await page.evaluate(() => {
        const entries = [...performance.getEntriesByType('navigation'), ...performance.getEntriesByType('resource')]
          .filter(entry => new URL(entry.name).origin === location.origin && !new URL(entry.name).pathname.startsWith('/api/'))
        return {
          requests: entries.length,
          encoded_body_bytes: entries.reduce((sum, entry) => sum + entry.encodedBodySize, 0),
          decoded_body_bytes: entries.reduce((sum, entry) => sum + entry.decodedBodySize, 0),
        }
      })
    } finally { await context.close() }
  }
  console.log(JSON.stringify(result))
} finally { await browser.close() }
