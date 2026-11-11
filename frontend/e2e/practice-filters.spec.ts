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
  opponent_rating: null, themes: ['fork', 'knight-fork'], status: 'new', box: 0, attempts: 0, correct: 0,
}
const stats = { total: 2, new: 2, due: 0, learning: 0, mastered: 0, next_due_at: null }
const answer: PracticeResult = {
  themes: ['fork', 'knight-fork'], correct: false, uci: null, san: null, best_uci: 'e2e4', best_san: 'e4',
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
      theme_counts: { fork: 1, 'knight-fork': 1 },
      items: endgame ? [{ ...item, phase: 'endgame' }] : [item],
      stats: { ...stats, total: endgame ? 1 : 2, new: attempted ? 0 : 1, learning: attempted ? 1 : 0 },
    }
    await route.fulfill({ json: queue })
  })
  await page.goto('/practice')
  await page.getByLabel('Game phase').selectOption('endgame')
  await expect(page.getByText('Move 1 · Endgame')).toBeVisible()
  await expect(page.getByText('1 position with one clear answer')).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Game phase')).toHaveValue('endgame')
  await page.getByRole('button', { name: 'Show answer' }).click()
  await expect(page.getByText('The move was e4')).toBeVisible()
  await expect(page.locator('dl > div').filter({ has: page.getByText('Learning', { exact: true }) }).locator('dd')).toHaveText('1')
  await expect.poll(() => requests.some(u => u.searchParams.get('phase') === 'endgame' && u.searchParams.get('limit') === '1')).toBe(true)
  expect(errors).toEqual([])
})

test('Practice theme filters show counts, persist and reveal themes only after the answer', async ({ page }) => {
  const requests: URL[] = []
  let attempted = false
  const errors = await fixture(page, async (route, url) => {
    if (route.request().method() === 'POST') {
      attempted = true
      await route.fulfill({ json: { ...answer, stats: { ...stats, total: 50, new: 10, learning: 40 } } })
      return
    }
    requests.push(url)
    const theme = url.searchParams.get('theme')
    const empty = theme === 'promotion'
    const queue: PracticeQueue = {
      theme_counts: { fork: 2, 'knight-fork': 1, promotion: 1 },
      items: empty ? [] : [item],
      stats: { ...stats, total: empty ? 0 : theme ? 1 : 2, new: empty || attempted ? 0 : 1, learning: attempted ? 1 : 0 },
    }
    await route.fulfill({ json: queue })
  })
  await page.goto('/practice')
  await expect(page.getByLabel('Tactical theme').locator('option[value="fork"]')).toHaveText('Fork (2)')
  await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveCount(0)
  await page.getByLabel('Tactical theme').selectOption('knight-fork')
  await expect(page.getByText('1 position with one clear answer')).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Tactical theme')).toHaveValue('knight-fork')
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByLabel('Tactical theme')).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390)
  // A wrong attempt returns the answer for retries, but must not show the theme yet.
  await playOnBoard(page, 'a2', 'a3')
  await expect(page.getByText('a3 isn’t it. Try again.')).toBeVisible()
  await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Show answer' }).click()
  await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveText('Themes: Fork, Knight fork')
  await expect.poll(() => requests.some(u => u.searchParams.get('theme') === 'knight-fork' && u.searchParams.get('limit') === '1')).toBe(true)
  await expect(page.locator('dl > div').filter({ has: page.getByText('Learning', { exact: true }) }).locator('dd')).toHaveText('1')
  await page.getByLabel('Tactical theme').selectOption('promotion')
  await expect(page.getByText('No positions match these filters')).toBeVisible()
  await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveCount(0)
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

async function playOnBoard(page: Page, from: string, to: string, orientation: 'white' | 'black' = 'white') {
  // The board is rebuilt when it becomes playable, so wait for the playable one before selecting
  // squares. Locator clicks also check stability and use its current position if the page scrolls.
  const target = page.locator('.cg-wrap.manipulable cg-board')
  await expect(target).toBeVisible()
  await target.scrollIntoViewIfNeeded()
  const board = await target.boundingBox()
  if (!board) throw new Error('Chessboard is not visible')
  for (const square of [from, to]) {
    const file = square.charCodeAt(0) - 97
    const rank = Number(square[1]) - 1
    await target.click({ position: {
      x: ((orientation === 'white' ? file : 7 - file) + 0.5) * board.width / 8,
      y: ((orientation === 'white' ? 7 - rank : rank) + 0.5) * board.height / 8,
    } })
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
  await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveText('Themes: Fork, Knight fork')
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
      eco: null, opening: null, recent_games: [], recent_plies: [], from: index, to: index + 1, orders: 1,
    }))
    const positions = nodes.map(n => ({ id: n.to, games: n.games, wins: n.wins, draws: n.draws, losses: n.losses, score: n.score, orders: n.orders }))
    const repertoire: Repertoire = { color: 'white', plies, merged: false, root: 0, games: 1, total_games: 1, wins: 1, draws: 0, losses: 0, nodes, positions, problems: [] }
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

test('Openings can merge move orders that reach the same position', async ({ page }) => {
  const merges: (string | null)[] = []
  const base = { mine: true, games: 1, wins: 1, draws: 0, losses: 0, score: 1, last_played: 0, book: 1, left_book: 0,
    errors: 0, blunders: 0, misses: 0, avg_loss: 0, eco: null, opening: null, recent_games: [7], recent_plies: [5], orders: 1 }
  const node = (path: string, from: number, to: number, extra: Partial<RepertoireNode> = {}): RepertoireNode => ({
    ...base, path, san: path.split(' ').at(-1)!, ply: path.split(' ').length, mine: path.split(' ').length % 2 === 1, from, to, ...extra })
  const errors = await fixture(page, async (route, url) => {
    const merged = url.searchParams.get('merge') === 'true'
    merges.push(url.searchParams.get('merge'))
    // Two orders into the Queen's Gambit Declined; merged, they share position 4 and its Nc3.
    const nodes = merged ? [
      node('d4', 0, 1), node('d4 Nf6', 1, 2), node('d4 Nf6 c4', 2, 3), node('d4 Nf6 c4 e6', 3, 4),
      node('c4', 0, 5), node('c4 e6', 5, 6), node('c4 e6 d4', 6, 7), node('c4 e6 d4 Nf6', 7, 4),
      node('d4 Nf6 c4 e6 Nc3', 4, 8, { games: 2, wins: 2, orders: 2, recent_games: [7, 8], recent_plies: [5, 5] }),
      // One of those games returns to position 4 via a third incoming edge.
      node('d4 Nf6 c4 e6 Nc3 Ng8', 8, 9), node('d4 Nf6 c4 e6 Nc3 Ng8 Nb1', 9, 10),
      node('d4 Nf6 c4 e6 Nc3 Ng8 Nb1 Nf6', 10, 4),
    ] : [
      node('d4', 0, 1), node('d4 Nf6', 1, 2), node('d4 Nf6 c4', 2, 3), node('d4 Nf6 c4 e6', 3, 4), node('d4 Nf6 c4 e6 Nc3', 4, 5),
      node('c4', 0, 6), node('c4 e6', 6, 7), node('c4 e6 d4', 7, 8), node('c4 e6 d4 Nf6', 8, 9), node('c4 e6 d4 Nf6 Nc3', 9, 10),
    ]
    const positions = [...new Map(nodes.map(n => [n.to, { id: n.to, games: n.games, wins: n.wins, draws: n.draws, losses: n.losses, score: n.score, orders: n.orders }])).values()]
    if (merged) Object.assign(positions.find(p => p.id === 4)!, { games: 2, wins: 2, orders: 3 })
    const repertoire: Repertoire = { color: 'white', plies: 16, merged, root: 0, games: 2, total_games: 2, wins: 2, draws: 0, losses: 0, nodes, positions, problems: [] }
    await route.fulfill({ json: repertoire })
  })
  await page.goto(`/openings?${new URLSearchParams({ line: 'c4 e6 d4 Nf6' })}`)
  await expect(page.getByText('1 of your games reached this position · you scored 100%')).toBeVisible()
  await page.getByLabel('Move orders').selectOption('1')
  await expect(page).toHaveURL(/merge=1/)
  // The line is kept, and the move from the shared position counts both games.
  await expect(page.getByLabel('Current line')).toContainText('2. d4Nf6')
  await expect(page.getByRole('button', { name: 'Nc3 2 100% 2 move orders' })).toBeVisible()
  await expect(page.getByText('2 of your games reached this position by 3 move orders · you scored 100%')).toBeVisible()
  await page.getByRole('button', { name: /^Nc3/ }).click()
  await expect(page.getByText('2 of your games reached this position by 2 move orders · you scored 100%')).toBeVisible()
  await expect(page.getByRole('link', { name: 'Latest game' })).toHaveAttribute('href', '/games/7?ply=5')
  expect(merges).toEqual([null, 'true'])
  expect(errors).toEqual([])
})

test('A slow first answer says so, and Skip still moves on (#29)', async ({ page }) => {
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') return // never answers
    await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
  })
  await page.clock.install()
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Checking your move…')).toBeVisible()
  await page.clock.fastForward(6_000)
  await expect(page.getByText(/taking longer than usual/)).toBeVisible()
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  await expect(page.getByText(/against Second opponent/)).toBeVisible()
  await expect(page.getByText('Find the best move, then keep going.')).toBeVisible()
  await expect(page.getByText(/taking longer than usual/)).toHaveCount(0)
  expect(errors).toEqual([])
})

test('A stuck engine check gives the board back after a timeout (#29)', async ({ page }) => {
  let checks = 0
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/engine/analyse') { checks++; return } // never answers
    if (route.request().method() === 'POST') {
      await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
      return
    }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.clock.install()
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await page.clock.runFor(500)
  await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
  // Bc4 isn't in the stored line, so the engine is asked, and it never answers.
  await playOnBoard(page, 'f1', 'c4')
  await expect(page.getByText('Checking that move with the engine…')).toBeVisible()
  await page.clock.fastForward(6_000)
  await expect(page.getByText(/taking longer than usual/)).toBeVisible()
  await page.clock.fastForward(5_000)
  await expect(page.getByRole('alert')).toContainText('The engine didn’t answer in time')
  await expect(page.getByText(/wasn’t counted as wrong/)).toBeVisible()
  // The board is usable again: the stored move is accepted.
  await playOnBoard(page, 'g1', 'f3')
  await page.clock.runFor(500)
  await expect(page.getByText('Good. Find the next move (3 of 3).')).toBeVisible()
  expect(checks).toBeGreaterThan(0)
  expect(errors).toEqual([])
})

test('The promotion picker sits over the board and can be cancelled (#29)', async ({ page }) => {
  const posted: unknown[] = []
  const promo: PracticeItem = { ...item, fen: '4k3/1P6/8/8/8/8/8/4K3 w - - 0 1', phase: 'endgame' }
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') {
      posted.push(route.request().postDataJSON())
      await route.fulfill({ json: { ...answer, correct: true, uci: 'b7b8q', san: 'b8=Q+', best_uci: 'b7b8q', best_san: 'b8=Q+', line_san: ['b8=Q+'] } })
      return
    }
    await route.fulfill({ json: { stats, items: [promo] } })
  })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/practice')
  await playOnBoard(page, 'b7', 'b8')
  const picker = page.getByRole('group', { name: 'Promote pawn' })
  await expect(picker).toBeInViewport()
  await expect(page.getByText('Choose a piece to promote to.')).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(picker).toHaveCount(0)
  expect(posted).toEqual([])
  await playOnBoard(page, 'b7', 'b8')
  await picker.getByRole('button', { name: 'Queen' }).click()
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  expect(posted).toEqual([{ uci: 'b7b8q', time_zone: expect.any(String) }])
  expect(errors).toEqual([])
})

test('The board still takes moves after the page shifts under it (#29)', async ({ page }) => {
  let posted = 0
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') { posted++; await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4'] } }); return }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.goto('/practice')
  const board = page.locator('cg-board')
  await expect(board).toBeVisible()
  // Hover the board so the library measures it, then move it down without a scroll or resize,
  // as a banner or a wrapping header row would.
  await board.hover()
  await page.evaluate(() => {
    const spacer = document.createElement('div')
    spacer.style.height = '137px'
    document.querySelector('main')!.prepend(spacer)
  })
  const box = (await board.boundingBox())!
  for (const square of ['e2', 'e4']) {
    await page.mouse.click(box.x + (square.charCodeAt(0) - 97 + 0.5) * box.width / 8, box.y + (8 - Number(square[1]) + 0.5) * box.height / 8)
  }
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  expect(posted).toBe(1)
  expect(errors).toEqual([])
})

test('Turning off "Play out the line" mid-puzzle applies from the next position', async ({ page }) => {
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') {
      await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
      return
    }
    await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
  })
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
  await page.getByRole('switch', { name: 'Play out the line' }).click()
  await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
  await expect(page.getByText('“Play out the line” changes from the next position.')).toBeVisible()
  await page.getByRole('button', { name: 'Skip', exact: true }).click()
  await expect(page.getByText(/against Second opponent/)).toBeVisible()
  await expect(page.getByText('Find the best move.', { exact: true })).toBeVisible()
  await expect(page.getByText(/changes from the next position/)).toHaveCount(0)
  expect(errors).toEqual([])
})

test('Hint is offered only on your move, not while the opponent replies', async ({ page }) => {
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') {
      await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
      return
    }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.clock.install()
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Your opponent replies…')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Hint' })).toHaveCount(0)
  await page.clock.runFor(500)
  await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Hint' })).toBeVisible()
  expect(errors).toEqual([])
})

test('Practice can be played from the keyboard by typing moves', async ({ page }) => {
  const posted: unknown[] = []
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') {
      posted.push(route.request().postDataJSON())
      await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3'] } })
      return
    }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.clock.install()
  await page.goto('/practice')
  const field = page.getByLabel('Type your move')
  await field.fill('Ke2')
  await field.press('Enter')
  await expect(page.getByText('Ke2 isn’t a legal move here.')).toBeVisible()
  expect(posted).toEqual([])
  await field.fill('e2e4')
  await field.press('Enter')
  await expect.poll(() => posted).toEqual([{ uci: 'e2e4', time_zone: expect.any(String) }])
  await page.clock.runFor(500)
  await expect(page.getByText('Good. Find the next move (2 of 2).')).toBeVisible()
  await field.fill('Nf3')
  await field.press('Enter')
  await expect(page.getByText('Solved the line on the first try')).toBeVisible()
  expect(errors).toEqual([])
})

test('A double-click on Show answer records one answer', async ({ page }) => {
  let posts = 0
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') { posts++; await new Promise(r => setTimeout(r, 300)); await route.fulfill({ json: answer }); return }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.goto('/practice')
  await page.getByRole('button', { name: 'Show answer' }).dblclick()
  await expect(page.getByText('The move was e4')).toBeVisible()
  expect(posts).toBe(1)
  expect(errors).toEqual([])
})

test('The board takes moves on the next position after it flips to the other colour', async ({ page }) => {
  const black: PracticeItem = { ...item, game_id: 102, ply: 2, color: 'black', fen: 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1', last_uci: 'e2e4', last_san: 'e4' }
  const posted: string[] = []
  const errors = await fixture(page, async (route, url) => {
    if (route.request().method() === 'POST') {
      const { uci } = route.request().postDataJSON() as { uci: string }
      const san = uci === 'e2e4' ? 'e4' : 'e5'
      posted.push(`${url.pathname.split('/').slice(-2).join('/')} ${uci}`)
      await route.fulfill({ json: { ...answer, correct: true, uci, san, best_uci: uci, best_san: san, line_san: [san] } })
      return
    }
    await route.fulfill({ json: { stats, items: [item, black] } })
  })
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  await page.getByRole('button', { name: 'Next', exact: true }).click()
  await expect(page.getByText('Black to move')).toBeVisible()
  await playOnBoard(page, 'e7', 'e5', 'black')
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  expect(posted).toEqual(['101/1 e2e4', '102/2 e7e5'])
  expect(errors).toEqual([])
})

test('The board takes moves after it was resized while locked', async ({ page }) => {
  let posted = 0
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') { posted++; await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4'] } }); return }
    await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 102 }] } })
  })
  await page.goto('/practice')
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  // The board is locked between positions. A size change now (a window resize, a scrollbar
  // appearing) makes the library rebuild it.
  await page.setViewportSize({ width: 1200, height: 800 })
  await page.getByRole('button', { name: 'Next', exact: true }).click()
  await expect(page.getByText('2 / 2')).toBeVisible()
  await playOnBoard(page, 'e2', 'e4')
  await expect(page.getByText('Solved on the first try')).toBeVisible()
  expect(posted).toBe(2)
  expect(errors).toEqual([])
})

test('A finished puzzle can be copied as PGN or opened on Lichess', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  const errors = await fixture(page, async route => {
    if (route.request().method() === 'POST') { await route.fulfill({ json: answer }); return }
    await route.fulfill({ json: { stats, items: [item] } })
  })
  await page.goto('/practice')
  await page.getByRole('button', { name: 'Show answer' }).click()
  await expect(page.getByText('The move was e4')).toBeVisible()
  await expect(page.getByRole('link', { name: 'Analyse on Lichess' }))
    .toHaveAttribute('href', 'https://lichess.org/analysis/rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR_w_KQkq_-_0_1?color=white')
  await page.getByRole('button', { name: 'Copy PGN' }).click()
  await expect(page.getByRole('button', { name: 'Copied' })).toBeVisible()
  const pgn = await page.evaluate(() => navigator.clipboard.readText())
  expect(pgn).toContain('[White "practice_fixture"]')
  expect(pgn).toContain('[Black "First opponent"]')
  expect(pgn).toContain('{In the game: a3. Best: e4.} 1. e4 e5 2. Nf3')
  expect(errors).toEqual([])
})
