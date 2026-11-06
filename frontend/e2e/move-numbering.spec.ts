import { expect, test, type Page } from '@playwright/test'
import { Chess } from 'chess.js'

test.use({ actionTimeout: 20_000 })

async function reviewPosition(page: Page, fen: string, sans: string | string[], analyzed = false) {
  const chess = new Chess(fen)
  const moves = (typeof sans === 'string' ? [sans] : sans).map((san, i) => {
    const before = chess.fen()
    const move = chess.move(san)
    return { ply: i + 1, color: move.color, san: move.san, uci: `${move.from}${move.to}${move.promotion ?? ''}`, fen_before: before,
      ...analyzed ? { classification: 'mistake', eval_after: 0,
        lines: [{ eval: '0.00', san: [move.color === 'b' ? 'Kg8' : 'Kg1'], depth: 12 }] } : {} }
  })
  const side = { username: 'Position fixture', rating: null, result: null, accuracy: null, chesscom_accuracy: null }
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/auth/session') {
      await route.fulfill({ json: { authenticated: true, auth_required: false, password_set: false } })
    } else if (url.pathname === '/api/accounts') {
      await route.fulfill({ json: [] })
    } else if (url.pathname === '/api/status') {
      await route.fulfill({ json: { accounts: 0, games: 1, analyzed: 0,
        engine: { name: 'Fixture', error: null }, sync: { current_account: null },
        queue: { queued: 0, running: 0, failed: 0, paused: false, eta_seconds: null } } })
    } else if (url.pathname === '/api/games/42') {
      await route.fulfill({ json: { analysis: analyzed ? {
        depth: 12, multipv: 1, engine: 'Fixture', analyzed_at: 1,
        accuracy: { white: 80, black: 80 }, acpl: { white: 20, black: 20 },
        counts: { white: {}, black: {} }, opening: {}, key_moments: moves.map(m => m.ply),
        evals: [0, ...moves.map(() => 0)], win_white: [50, ...moves.map(() => 50)],
      } : null,
        game: { id: 42, white: side, black: side, result: '*', rules: 'chess', user_color: 'white',
          opening: {}, job: null, initial_fen: fen, final_fen: chess.fen() },
        moves } })
    } else if (url.pathname === '/api/engine/analyse') {
      await route.fulfill({ json: { fen, lines: [] } })
    } else await route.fulfill({ json: [] })
  })
  await page.goto('/games/42')
  await expect(page.locator('cg-board')).toBeVisible()
  await expect(page.getByText('42.', { exact: true })).toBeVisible()
}

async function playMove(page: Page, from: string, to: string) {
  const board = page.locator('cg-board')
  await board.evaluate(el => el.scrollIntoView({ block: 'center' }))
  const size = await board.boundingBox()
  if (!size) throw new Error('Chessboard is not visible')
  const squarePosition = (square: string) => ({
    x: (square.charCodeAt(0) - 97 + 0.5) * size.width / 8,
    y: (8 - Number(square[1]) + 0.5) * size.height / 8,
  })
  await board.click({ position: squarePosition(from) })
  await expect(board.locator('square.selected')).toBeVisible()
  await board.click({ position: squarePosition(to) })
}

test('a white FEN move 42 keeps its number in the move list and an underpromotion variation', async ({ page }) => {
  await reviewPosition(page, '7k/P7/8/8/8/8/8/7K w - - 0 42', 'a8=Q+')
  await playMove(page, 'a7', 'a8')
  await page.getByRole('button', { name: 'Knight', exact: true }).click()
  await expect(page.getByText('from move 42', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'a8=N', exact: true })).toBeVisible()
})

test('a black FEN move 42 keeps its turn in a variation label', async ({ page }) => {
  await reviewPosition(page, '7k/8/8/8/8/8/8/7K b - - 0 42', 'Kh7')
  await playMove(page, 'h8', 'g8')
  await expect(page.getByText('from move 42…', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Kg8', exact: true })).toBeVisible()
})

test('analyzed FEN review preserves black 42 and white 43 in details, key moments, graphs and engine variations', async ({ page }) => {
  await reviewPosition(page, '7k/8/8/8/8/8/8/7K b - - 0 42', ['Kh7', 'Kh2'], true)
  const black = page.getByRole('button', { name: /42… Kh7$/ })
  const white = page.getByRole('button', { name: /43\. Kh2$/ })
  await expect(black).toBeVisible()
  await expect(white).toBeVisible()
  await black.click()
  await expect(page.locator('[aria-live="polite"]')).toContainText('42… Kh7')

  const graph = page.locator('svg[aria-label="Evaluation graph"]:visible')
  const box = await graph.boundingBox()
  if (!box) throw new Error('Evaluation graph is not visible')
  await graph.hover({ position: { x: box.width / 2, y: box.height / 2 } })
  await expect(page.getByText('42… Kh7 · 0.00', { exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'Kg8', exact: true }).click()
  await expect(page.getByText('from move 42…', { exact: true })).toBeVisible()
  await expect(page.getByText('from move 42…', { exact: true }).locator('..')).toContainText('Engine line')
  await page.getByRole('button', { name: 'Back to game', exact: true }).click()
  await white.click()
  await expect(page.locator('[aria-live="polite"]')).toContainText('43. Kh2')
  await page.getByRole('button', { name: 'Kg1', exact: true }).click()
  await expect(page.getByText('from move 43', { exact: true })).toBeVisible()
})
