import { expect, test, type Page, type Route } from '@playwright/test'
import { Chess } from 'chess.js'
import type { Account, Report, Repertoire, RepertoireNode } from '../src/lib/types'

test.use({ actionTimeout: 20_000 })

const account: Account = {
  id: 1, username: 'review_fixture', display_name: 'Review fixture', avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 1, last_sync_at: 1,
  sync_status: 'idle', sync_error: null, sync_enabled: false, auto_analyze: false,
  analyze_history: false, import_since: null, games: 1, analyzed: 0, queued: 0,
}

/** All API data is synthetic; the isolated server only supplies the built interface. */
async function fixture(page: Page, handle: (route: Route, url: URL) => Promise<void>) {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (url.pathname === '/api/accounts') {
      await route.fulfill({ json: [account] })
    } else if (url.pathname === '/api/status') {
      await route.fulfill({ json: { data_revision: 1, accounts: 1, games: 1, analyzed: 0,
        engine: { name: 'Fixture', error: null, path: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null } } })
    } else {
      await handle(route, url)
    }
  })
  return errors
}

/** An unanalysed game, so the live engine panel is always shown. */
function game(sans: string[], clocks: (number | null)[] = []) {
  const chess = new Chess()
  const moves = sans.map((san, i) => {
    const before = chess.fen()
    const m = chess.move(san)
    return { ply: i + 1, color: m.color, san: m.san, uci: `${m.from}${m.to}${m.promotion ?? ''}`, fen_before: before, clock: clocks[i] ?? null }
  })
  const side = { username: 'Fixture player', rating: null, result: null, accuracy: null, chesscom_accuracy: null }
  return { analysis: null, moves, game: { id: 42, white: side, black: { ...side, username: 'Fixture opponent' }, result: '*', rules: 'chess',
    user_color: 'white', opening: {}, job: null, time_control: '180+2', time_class: 'blitz', initial_fen: null, final_fen: chess.fen() } }
}

async function playMove(page: Page, from: string, to: string) {
  const board = page.locator('.cg-wrap.manipulable cg-board')
  await expect(board).toBeVisible()
  await board.scrollIntoViewIfNeeded()
  const size = await board.boundingBox()
  if (!size) throw new Error('Chessboard is not visible')
  const at = (square: string) => ({ x: (square.charCodeAt(0) - 97 + 0.5) * size.width / 8, y: (8 - Number(square[1]) + 0.5) * size.height / 8 })
  await board.click({ position: at(from) })
  await expect(board.locator('square.selected')).toBeVisible()
  await board.click({ position: at(to) })
}

test('a live engine move clicked inside a variation continues it', async ({ page }) => {
  const afterD4 = new Chess(); afterD4.move('d4')
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/games/42') await route.fulfill({ json: game(['e4', 'e5']) })
    else if (url.pathname === '/api/engine/analyse') {
      const { fen } = route.request().postDataJSON() as { fen: string }
      const lines = fen === afterD4.fen() ? [{ cp: 20, eval: '+0.20', win_white: 52, uci: ['d7d5', 'c2c4'], san: ['d5', 'c4'], depth: 20 }] : []
      await route.fulfill({ json: { fen, lines } })
    } else await route.fulfill({ json: [] })
  })
  await page.goto('/games/42')
  await playMove(page, 'd2', 'd4')
  await expect(page.getByText('Your variation')).toBeVisible()
  await page.getByRole('button', { name: 'c4', exact: true }).click()
  // The variation keeps 1. d4 and adds the engine's d5 c4 after it.
  const panel = page.getByText('from move 1', { exact: true }).locator('xpath=../../..')
  await expect(panel.getByRole('button')).toHaveText(['Back to game', 'd4', 'd5', 'c4'])
  expect(errors).toEqual([])
})

test('a finished game shows how it ended in the live engine, and clocks start at the base time', async ({ page }) => {
  const mated = game(['f3', 'e5', 'g4', 'Qh4#'], [178, 177, 170, 175])
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/games/42') await route.fulfill({ json: mated })
    else if (url.pathname === '/api/engine/analyse') {
      const { fen } = route.request().postDataJSON() as { fen: string }
      // What the server answers for a finished position: one empty line at depth 0.
      const lines = fen === mated.game.final_fen
        ? [{ cp: -100000, eval: '#', win_white: 0, uci: [], san: [], depth: 0, mate: 0 }]
        : [{ cp: 0, eval: '+0.00', win_white: 50, uci: ['e2e4'], san: ['e4'], depth: 20 }]
      await route.fulfill({ json: { fen, lines } })
    } else await route.fulfill({ json: [] })
  })
  await page.goto('/games/42?ply=0')
  await expect(page.getByText('3:00', { exact: true })).toHaveCount(2)
  await page.goto('/games/42?ply=1')
  // White has moved; Black has not and still shows the base time.
  await expect(page.getByText('2:58', { exact: true })).toBeVisible()
  await expect(page.getByText('3:00', { exact: true })).toHaveCount(1)
  await page.goto('/games/42?ply=4')
  // The board marks the mated king with its own "#" badge; the engine panel has no "#" chip.
  const engine = page.locator('div.rounded-lg').filter({ hasText: 'Live engine' }).last()
  await expect(engine.getByText('Checkmate. Black wins.')).toBeVisible()
  await expect(engine.getByText('#', { exact: true })).toHaveCount(0)
  expect(errors).toEqual([])
})

test('deleting one report does not spin or close another row’s confirmation', async ({ page }) => {
  const report = (id: number): Report => ({ id, account_id: 1, title: `Fixture report ${id}`, status: 'ready', total_games: 3, analyzed_games: 3,
    created_at: 1, filters: {} } as unknown as Report)
  let pending: Route | undefined
  let reports = [report(1), report(2)]
  const errors = await fixture(page, async (route, url) => {
    if (route.request().method() === 'DELETE') { pending = route; return }
    if (url.pathname === '/api/accounts/1/reports') await route.fulfill({ json: reports })
    else await route.fulfill({ json: [] })
  })
  await page.goto('/reports')
  const row = (id: number) => page.locator('div.flex.items-center.gap-4').filter({ hasText: `Fixture report ${id}` })
  await row(1).getByRole('button', { name: 'Delete report' }).click()
  await row(1).getByRole('button', { name: 'Delete', exact: true }).click()
  await expect.poll(() => !!pending).toBe(true)
  await row(2).getByRole('button', { name: 'Delete report' }).click()
  await expect(row(2).getByRole('button', { name: 'Delete', exact: true })).toBeEnabled()
  await expect(row(1).getByRole('button', { name: 'Delete', exact: true })).toBeDisabled()
  reports = [report(2)]
  await pending!.fulfill({ json: { deleted: true } })
  await expect(row(1)).toHaveCount(0)
  // Report 2's confirmation stays open after report 1 is gone.
  await expect(row(2).getByRole('button', { name: 'Delete', exact: true })).toBeEnabled()
  expect(errors).toEqual([])
})

test('a pawn can underpromote on the Openings board', async ({ page }) => {
  // A synthetic tree in which games went 5. bxa8=N.
  const line = ['e4', 'd5', 'exd5', 'c6', 'dxc6', 'Nf6', 'cxb7', 'Bd7', 'bxa8=N']
  const node = (i: number): RepertoireNode => ({
    path: line.slice(0, i + 1).join(' '), san: line[i], ply: i + 1, mine: i % 2 === 0, games: 2, wins: 1, draws: 0, losses: 1, score: 0.5,
    last_played: 1, book: 0, left_book: 0, errors: 0, blunders: 0, misses: 0, avg_loss: null, eco: null, opening: null,
    recent_games: [], recent_plies: [], from: i, to: i + 1, orders: 1,
  })
  const tree: Repertoire = { color: 'white', plies: 16, merged: false, root: 0, games: 2, total_games: 2, wins: 1, draws: 0, losses: 1,
    nodes: line.map((_, i) => node(i)), problems: [],
    positions: line.map((_, i) => ({ id: i + 1, games: 2, wins: 1, draws: 0, losses: 1, score: 0.5, orders: 1 })) }
  const errors = await fixture(page, async (route, url) => {
    if (url.pathname === '/api/accounts/1/repertoire') await route.fulfill({ json: tree })
    else await route.fulfill({ json: [] })
  })
  await page.goto(`/openings?line=${encodeURIComponent(line.slice(0, -1).join(' '))}`)
  await expect(page.getByText('Your moves from here.')).toBeVisible()
  await playMove(page, 'b7', 'a8')
  await page.getByRole('group', { name: 'Promote pawn' }).getByRole('button', { name: 'Knight' }).click()
  await expect(page.getByLabel('Current line').getByRole('button').last()).toHaveText('5. bxa8=N')
  await expect(page.getByText(/No game continued/)).toHaveCount(0)
  // Cancelling puts the pawn back without moving along the tree.
  await page.getByRole('button', { name: 'Back one move' }).click()
  await playMove(page, 'b7', 'a8')
  await page.getByRole('group', { name: 'Promote pawn' }).getByRole('button', { name: 'Cancel' }).click()
  await expect(page.getByLabel('Current line').getByRole('button').last()).toHaveText('Bd7')
  expect(errors).toEqual([])
})
