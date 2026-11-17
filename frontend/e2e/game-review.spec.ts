import { expect, test, type Page } from '@playwright/test'
import { Chess } from 'chess.js'
import { account as makeAccount, status as makeStatus, status, mockApi, playMove } from './support'

test.describe(() => {
  test.use({ actionTimeout: 20_000 })
  const apiOptions = { accounts: [makeAccount({ username: 'review_fixture', display_name: 'Review fixture', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 1 })], status: makeStatus({ games: 1, engine: { name: 'Fixture', error: null, path: null } }) }
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

  test('a live engine move clicked inside a variation continues it', async ({ page }) => {
    const afterD4 = new Chess(); afterD4.move('d4')
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/games/42') await route.fulfill({ json: game(['e4', 'e5']) })
      else if (url.pathname === '/api/engine/analyse') {
        const { fen } = route.request().postDataJSON() as { fen: string }
        const lines = fen === afterD4.fen() ? [{ cp: 20, eval: '+0.20', win_white: 52, uci: ['d7d5', 'c2c4'], san: ['d5', 'c4'], depth: 20 }] : []
        await route.fulfill({ json: { fen, lines } })
      } else await route.fulfill({ json: [] })
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/games/42') await route.fulfill({ json: mated })
      else if (url.pathname === '/api/engine/analyse') {
        const { fen } = route.request().postDataJSON() as { fen: string }
        // What the server answers for a finished position: one empty line at depth 0.
        const lines = fen === mated.game.final_fen
          ? [{ cp: -100000, eval: '#', win_white: 0, uci: [], san: [], depth: 0, mate: 0 }]
          : [{ cp: 0, eval: '+0.00', win_white: 50, uci: ['e2e4'], san: ['e4'], depth: 20 }]
        await route.fulfill({ json: { fen, lines } })
      } else await route.fulfill({ json: [] })
    }, undefined, apiOptions)
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
})

test.describe(() => {
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
    await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/games/42') {
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
    }, undefined, { accounts: [], status: status({ data_revision: undefined, accounts: 0, games: 1, engine: { name: 'Fixture', error: null } }) })
    await page.goto('/games/42')
    await expect(page.locator('cg-board')).toBeVisible()
    await expect(page.getByText('42.', { exact: true })).toBeVisible()
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
})
