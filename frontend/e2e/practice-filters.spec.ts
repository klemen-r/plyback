import { expect, test, type Page, type Route } from '@playwright/test'
import type { Account, PracticeItem, PracticeQueue, PracticeResult, Repertoire, RepertoireNode } from '../src/lib/types'

const account: Account = {
  id: 1, username: 'practice_fixture', display_name: 'Practice fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: false, auto_analyze: false,
  analyze_history: false, import_since: null, games: 2, analyzed: 2, queued: 0,
}
const item: PracticeItem = {
  game_id: 101, ply: 1, fen: 'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1',
  color: 'white', move_number: 1, classification: 'mistake', phase: 'opening', ep_loss: 0.2,
  last_uci: null, last_san: null, end_time: null, time_class: 'rapid', opponent: 'First opponent',
  opponent_rating: null, status: 'new', box: 0, attempts: 0, correct: 0,
}
const stats = { total: 2, new: 2, due: 0, learning: 0, mastered: 0, next_due_at: null }
const answer: PracticeResult = {
  correct: false, uci: null, san: null, best_uci: 'e2e4', best_san: 'e4',
  line_san: ['e4', 'e5', 'Nf3'], game_san: 'a3', explanation: null, box: 0,
  due_at: Math.floor(Date.now() / 1000) + 600, stats: { ...stats, new: 1, learning: 1 },
}

/** All API data is synthetic; the isolated server only supplies the built interface. */
async function fixture(page: Page, handle: (route: Route, url: URL) => Promise<void>, revision = () => 1) {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (url.pathname === '/api/accounts') {
      await route.fulfill({ json: [account] })
    } else if (url.pathname === '/api/status') {
      await route.fulfill({ json: {
        data_revision: revision(), accounts: 1, games: 2, analyzed: 2,
        engine: { name: 'Fixture engine', error: null, path: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null },
      } })
    } else {
      await handle(route, url)
    }
  })
  return errors
}

test('Practice phase filters persist and keep the answer statistics scoped', async ({ page }) => {
  const requests: URL[] = []
  let attempted = false
  const errors = await fixture(page, async (route, url) => {
    if (route.request().method() === 'POST') {
      attempted = true
      // Account totals intentionally differ from this phase's one position.
      await route.fulfill({ json: { ...answer, stats: { ...stats, total: 50 } } })
      return
    }
    requests.push(url)
    const endgame = url.searchParams.get('phase') === 'endgame'
    const queue: PracticeQueue = {
      items: endgame ? [{ ...item, phase: 'endgame' }] : [item],
      stats: { ...stats, total: endgame ? 1 : 2, new: attempted ? 0 : 1, learning: attempted ? 1 : 0 },
    }
    await route.fulfill({ json: queue })
  })
  await page.goto('/practice')
  await page.getByLabel('Game phase').selectOption('endgame')
  await expect(page.getByText('Move 1 · Endgame')).toBeVisible()
  await expect(page.getByText('1 positions with one clear answer')).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Game phase')).toHaveValue('endgame')
  await page.getByRole('button', { name: 'Show answer' }).click()
  await expect(page.getByText('The move was e4')).toBeVisible()
  await expect(page.locator('dl > div').filter({ has: page.getByText('Learning', { exact: true }) }).locator('dd')).toHaveText('1')
  await expect.poll(() => requests.some(u => u.searchParams.get('phase') === 'endgame' && u.searchParams.get('limit') === '1')).toBe(true)
  expect(errors).toEqual([])
})

test('A delayed answer cannot reveal or replace the puzzle selected with Skip', async ({ page }) => {
  let pending: Route | undefined
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') { pending = route; return }
    await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
  })
  await page.goto('/practice')
  await page.getByRole('button', { name: 'Show answer' }).click()
  await expect.poll(() => !!pending).toBe(true)
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  await expect(page.getByText(/against Second opponent/)).toBeVisible()
  const response = page.waitForResponse(r => r.request().method() === 'POST')
  await pending!.fulfill({ json: answer })
  await response
  await expect(page.getByRole('button', { name: 'Show answer' })).toBeVisible()
  await expect(page.getByText('The move was e4')).toHaveCount(0)
  await expect(page.getByText('2 / 2', { exact: true })).toBeVisible()
  expect(errors).toEqual([])
})

test('An active Practice batch survives review updates and an empty batch replenishes', async ({ page }) => {
  let revision = 1
  let empty = false
  let requests = 0
  const errors = await fixture(page, async route => {
    requests++
    await route.fulfill({ json: { stats: empty ? { ...stats, total: 0, new: 0 } : stats,
      items: empty ? [] : [{ ...item, opponent: revision > 1 ? 'New opponent' : 'First opponent' }] } })
  }, () => revision)
  await page.clock.install()
  await page.goto('/practice')
  await expect(page.getByText(/against First opponent/)).toBeVisible()
  expect(requests).toBe(1)
  revision = 2
  const updated = page.waitForResponse(async response => response.url().endsWith('/api/status') && (await response.json()).data_revision === 2)
  await page.clock.fastForward(20_000)
  await updated
  await expect(page.getByText(/against First opponent/)).toBeVisible()
  expect(requests).toBe(1)
  empty = true
  await page.getByLabel('Game phase').selectOption('middlegame')
  await expect(page.getByText('No positions match these filters')).toBeVisible()
  empty = false
  revision = 3
  await page.clock.fastForward(20_000)
  await expect(page.getByText(/against New opponent/)).toBeVisible()
  expect(requests).toBe(3)
  expect(errors).toEqual([])
})

async function playOnBoard(page: Page, from: string, to: string) {
  await page.locator('cg-board').evaluate(el => el.scrollIntoView({ block: 'center' }))
  const board = await page.locator('cg-board').boundingBox()
  if (!board) throw new Error('Chessboard is not visible')
  for (const square of [from, to]) {
    await page.mouse.click(board.x + (square.charCodeAt(0) - 97 + 0.5) * board.width / 8,
      board.y + (8 - Number(square[1]) + 0.5) * board.height / 8)
  }
}

test('An accepted alternative continues the puzzle with the engine line', async ({ page }) => {
  const engineLine = (uci: string[], san: string[], win: number) => ({ cp: 30, eval: '+0.3', win_white: win, uci, san, depth: 14 })
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/engine/analyse') {
      const { fen } = route.request().postDataJSON() as { fen: string }
      const afterE5 = fen.startsWith('rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w')
      await route.fulfill({ json: { fen, lines: afterE5
        ? [engineLine(['g1f3', 'b8c6'], ['Nf3', 'Nc6'], 56), engineLine(['f1c4', 'g8f6', 'd2d3', 'f8c5'], ['Bc4', 'Nf6', 'd3', 'Bc5'], 54)]
        : [engineLine(['d2d3'], ['d3'], 55)] } })
    } else if (route.request().method() === 'POST') {
      await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
    } else {
      await route.fulfill({ json: { stats, items: [item] } })
    }
  })
  await page.goto('/practice')
  await expect(page.getByRole('switch', { name: 'Play out the line' })).toHaveAttribute('aria-checked', 'true')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
  // Bc4 isn't in the stored line, but the engine rates it about as well as Nf3.
  await playOnBoard(page, 'f1', 'c4')
  await expect(page.getByText(/Bc4 works too/)).toBeVisible()
  await expect(page.getByText('Good. Find the next move (3 of 3).')).toBeVisible()
  // The opponent replied with the engine's Nf6, so the next move is checked against that line.
  await playOnBoard(page, 'd2', 'd3')
  await expect(page.getByText('Solved the line on the first try')).toBeVisible()
  await expect(page.getByText('Engine line, with your alternative')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Nf6', exact: true })).toBeVisible()
  expect(errors).toEqual([])
})

test('Openings depth preserves the line and distinguishes a depth boundary from missing games', async ({ page }) => {
  const sans = ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6', 'Ba4', 'Nf6', 'O-O', 'Be7', 'Re1', 'b5', 'Bb3', 'd6', 'c3', 'O-O', 'h3']
  const requestedDepths: number[] = []
  const errors = await fixture(page, async (route, url) => {
    const plies = Number(url.searchParams.get('plies') ?? '16')
    requestedDepths.push(plies)
    const nodes: RepertoireNode[] = sans.slice(0, plies).map((san, index) => ({
      path: sans.slice(0, index + 1).join(' '), san, ply: index + 1, mine: index % 2 === 0,
      games: 1, wins: 1, draws: 0, losses: 0, score: 1, last_played: 0,
      book: 1, left_book: 0, errors: 0, blunders: 0, misses: 0, avg_loss: 0,
      eco: null, opening: null, recent_games: [],
    }))
    const repertoire: Repertoire = { color: 'white', plies, games: 1, total_games: 1, wins: 1, draws: 0, losses: 0, nodes, problems: [] }
    await route.fulfill({ json: repertoire })
  })
  await page.goto(`/openings?${new URLSearchParams({ line: sans.join(' ') })}`)
  await expect(page.getByText('The tree covers the first 8 moves.')).toBeVisible()
  await expect(page.getByRole('status')).toContainText('Choose a deeper tree to continue')
  await page.getByRole('button', { name: 'Explore to move 15' }).click()
  await expect(page.getByLabel('Current line')).toContainText('9. h3')
  await expect(page.getByLabel('Tree depth')).toHaveValue('30')
  await expect(page).toHaveURL(/plies=30/)
  await page.reload()
  await expect(page.getByLabel('Current line')).toContainText('9. h3')
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByLabel('Tree depth')).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390)
  expect(requestedDepths).toEqual([16, 30, 30])
  expect(errors).toEqual([])
})
