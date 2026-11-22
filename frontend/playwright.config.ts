import { defineConfig, devices } from '@playwright/test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const scripts = path.resolve(__dirname, '..', 'scripts')

export default defineConfig({
  testDir: './e2e',
  timeout: 180_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [['list']],
  use: { baseURL: 'http://127.0.0.1:8790', trace: 'retain-on-failure', viewport: { width: 1440, height: 900 } },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
  webServer: [
    { command: `node "${path.join(scripts, 'fixture_server.mjs')}" --port 8799`, url: 'http://127.0.0.1:8799/pub/player/fixture_player', reuseExistingServer: false, timeout: 60_000 },
    { command: `node "${path.join(scripts, 'e2e_server.mjs')}"`, url: 'http://127.0.0.1:8790/api/health', reuseExistingServer: false, timeout: 90_000 },
  ],
})
