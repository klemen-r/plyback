import { expect, test, type Page, type Route } from '@playwright/test'
import type { PracticeItem, PracticeQueue, PracticeResult } from '../src/lib/types'
import { account as makeAccount, status as makeStatus, mockApi, playMove } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'practice_fixture', display_name: 'Practice fixture', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ games: 2, analyzed: 2, engine: { name: 'Fixture engine', error: null, path: null } }) }
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
  /** The piece class ('white bishop') shown on a square of the white-oriented board, or ''. */
  async function pieceOn(page: Page, square: string) {
    return page.locator('cg-board').first().evaluate((board, sq) => {
      const box = board.getBoundingClientRect()
      const file = sq.charCodeAt(0) - 97
      const rank = Number(sq[1]) - 1
      const x = box.left + (file + 0.5) * box.width / 8
      const y = box.top + (7 - rank + 0.5) * box.height / 8
      for (const piece of board.querySelectorAll('piece')) {
        if (piece.classList.contains('ghost') || piece.classList.contains('fading')) continue
        const r = piece.getBoundingClientRect()
        if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) {
          return [...piece.classList].filter(c => c === 'white' || c === 'black' || ['king', 'queen', 'rook', 'bishop', 'knight', 'pawn'].includes(c)).join(' ')
        }
      }
      return ''
    }, square)
  }

  test('Practice phase filters persist and keep the answer statistics scoped', async ({ page }) => {
    const requests: URL[] = []
    let attempted = false
    const { errors } = await mockApi(page, async (route, url) => {
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
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async (route, url) => {
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
    }, undefined, apiOptions)
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
    await playMove(page, 'a2', 'a3')
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
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') { pending = route; return }
      await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async route => {
      requests++
      await route.fulfill({ json: { stats: empty ? { ...stats, total: 0, new: 0 } : stats,
        items: empty ? [] : [{ ...item, opponent: revision > 1 ? 'New opponent' : 'First opponent' }] } })
    }, () => revision, apiOptions)
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

  test('An accepted alternative continues the puzzle with the engine line', async ({ page }) => {
    const engineLine = (uci: string[], san: string[], win: number) => ({ cp: 30, eval: '+0.3', win_white: win, uci, san, depth: 14 })
    const { errors } = await mockApi(page, async (route, url) => {
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
    }, undefined, apiOptions)
    await page.goto('/practice')
    await expect(page.getByRole('switch', { name: 'Play out the line' })).toHaveAttribute('aria-checked', 'true')
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
    // Bc4 isn't in the stored line, but the engine rates it about as well as Nf3.
    await playMove(page, 'f1', 'c4')
    await expect(page.getByText(/Bc4 works too/)).toBeVisible()
    await expect(page.getByText('Good. Find the next move (3 of 3).')).toBeVisible()
    // The opponent replied with the engine's Nf6, so the next move is checked against that line.
    await playMove(page, 'd2', 'd3')
    await expect(page.getByText('Solved the line on the first try')).toBeVisible()
    await expect(page.getByLabel('Puzzle themes', { exact: true })).toHaveText('Themes: Fork, Knight fork')
    await expect(page.getByText('Engine line, with your alternative')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Nf6', exact: true })).toBeVisible()
    expect(errors).toEqual([])
  })

  test('A slow first answer says so, and Skip still moves on', async ({ page }) => {
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') return // never answers
      await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
    }, undefined, apiOptions)
    await page.clock.install()
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Checking your move…')).toBeVisible()
    await page.clock.fastForward(6_000)
    await expect(page.getByText(/taking longer than usual/)).toBeVisible()
    await page.getByRole('button', { name: 'Skip', exact: true }).click()
    await expect(page.getByText(/against Second opponent/)).toBeVisible()
    await expect(page.getByText('Find the best move, then keep going.')).toBeVisible()
    await expect(page.getByText(/taking longer than usual/)).toHaveCount(0)
    expect(errors).toEqual([])
  })

  test('A stuck engine check gives the board back after a timeout', async ({ page }) => {
    let checks = 0
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/engine/analyse') { checks++; return } // never answers
      if (route.request().method() === 'POST') {
        await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
    await page.clock.install()
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await page.clock.runFor(500)
    await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
    // Bc4 isn't in the stored line, so the engine is asked, and it never answers.
    await playMove(page, 'f1', 'c4')
    await expect(page.getByText('Checking that move with the engine…')).toBeVisible()
    await page.clock.fastForward(6_000)
    await expect(page.getByText(/taking longer than usual/)).toBeVisible()
    await page.clock.fastForward(5_000)
    await expect(page.getByRole('alert')).toContainText('The engine didn’t answer in time')
    await expect(page.getByText(/wasn’t counted as wrong/)).toBeVisible()
    // The board is usable again: the stored move is accepted.
    await playMove(page, 'g1', 'f3')
    await page.clock.runFor(500)
    await expect(page.getByText('Good. Find the next move (3 of 3).')).toBeVisible()
    expect(checks).toBeGreaterThan(0)
    expect(errors).toEqual([])
  })

  test('The promotion picker sits over the board and can be cancelled', async ({ page }) => {
    const posted: unknown[] = []
    const promo: PracticeItem = { ...item, fen: '4k3/1P6/8/8/8/8/8/4K3 w - - 0 1', phase: 'endgame' }
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') {
        posted.push(route.request().postDataJSON())
        await route.fulfill({ json: { ...answer, correct: true, uci: 'b7b8q', san: 'b8=Q+', best_uci: 'b7b8q', best_san: 'b8=Q+', line_san: ['b8=Q+'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [promo] } })
    }, undefined, apiOptions)
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/practice')
    await playMove(page, 'b7', 'b8')
    const picker = page.getByRole('group', { name: 'Promote pawn' })
    await expect(picker).toBeInViewport()
    await expect(page.getByText('Choose a piece to promote to.')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(picker).toHaveCount(0)
    expect(posted).toEqual([])
    await playMove(page, 'b7', 'b8')
    await picker.getByRole('button', { name: 'Queen' }).click()
    await expect(page.getByText('Solved on the first try')).toBeVisible()
    expect(posted).toEqual([{ uci: 'b7b8q', time_zone: expect.any(String) }])
    expect(errors).toEqual([])
  })

  test('The board still takes moves after the page shifts under it', async ({ page }) => {
    let posted = 0
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') { posted++; await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4'] } }); return }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') {
        await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 202, opponent: 'Second opponent' }] } })
    }, undefined, apiOptions)
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
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
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') {
        await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
    await page.clock.install()
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Your opponent replies…')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Hint' })).toHaveCount(0)
    await page.clock.runFor(500)
    await expect(page.getByText('Good. Find the next move (2 of 3).')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Hint' })).toBeVisible()
    expect(errors).toEqual([])
  })

  test('Practice can be played from the keyboard by typing moves', async ({ page }) => {
    const posted: unknown[] = []
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') {
        posted.push(route.request().postDataJSON())
        await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') { posts++; await new Promise(r => setTimeout(r, 300)); await route.fulfill({ json: answer }); return }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
    await page.goto('/practice')
    await page.getByRole('button', { name: 'Show answer' }).dblclick()
    await expect(page.getByText('The move was e4')).toBeVisible()
    expect(posts).toBe(1)
    expect(errors).toEqual([])
  })

  test('The board takes moves on the next position after it flips to the other colour', async ({ page }) => {
    const black: PracticeItem = { ...item, game_id: 102, ply: 2, color: 'black', fen: 'rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1', last_uci: 'e2e4', last_san: 'e4' }
    const posted: string[] = []
    const { errors } = await mockApi(page, async (route, url) => {
      if (route.request().method() === 'POST') {
        const { uci } = route.request().postDataJSON() as { uci: string }
        const san = uci === 'e2e4' ? 'e4' : 'e5'
        posted.push(`${url.pathname.split('/').slice(-2).join('/')} ${uci}`)
        await route.fulfill({ json: { ...answer, correct: true, uci, san, best_uci: uci, best_san: san, line_san: [san] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item, black] } })
    }, undefined, apiOptions)
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Solved on the first try')).toBeVisible()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await expect(page.getByText('Black to move')).toBeVisible()
    await playMove(page, 'e7', 'e5', 'black')
    await expect(page.getByText('Solved on the first try')).toBeVisible()
    expect(posted).toEqual(['101/1 e2e4', '102/2 e7e5'])
    expect(errors).toEqual([])
  })

  test('The board takes moves after it was resized while locked', async ({ page }) => {
    let posted = 0
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') { posted++; await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4'] } }); return }
      await route.fulfill({ json: { stats, items: [item, { ...item, game_id: 102 }] } })
    }, undefined, apiOptions)
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Solved on the first try')).toBeVisible()
    // The board is locked between positions. A size change now (a window resize, a scrollbar
    // appearing) makes the library rebuild it.
    await page.setViewportSize({ width: 1200, height: 800 })
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await expect(page.getByText('2 / 2')).toBeVisible()
    await playMove(page, 'e2', 'e4')
    await expect(page.getByText('Solved on the first try')).toBeVisible()
    expect(posted).toBe(2)
    expect(errors).toEqual([])
  })

  test('A finished puzzle can be copied as PGN or opened on Lichess', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    const { errors } = await mockApi(page, async route => {
      if (route.request().method() === 'POST') { await route.fulfill({ json: answer }); return }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
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

  test('Show answer during an engine check drops the tried move from the board', async ({ page }) => {
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/engine/analyse') return // never answers
      if (route.request().method() === 'POST') {
        await route.fulfill({ json: { ...answer, correct: true, uci: 'e2e4', san: 'e4', line_san: ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6'] } })
        return
      }
      await route.fulfill({ json: { stats, items: [item] } })
    }, undefined, apiOptions)
    await page.clock.install()
    await page.goto('/practice')
    await playMove(page, 'e2', 'e4')
    await page.clock.runFor(500)
    await playMove(page, 'f1', 'c4')
    await expect(page.getByText('Checking that move with the engine…')).toBeVisible()
    await page.getByRole('button', { name: 'Show answer' }).click()
    await expect(page.getByText('The move was e4')).toBeVisible()
    // The revealed line starts from the puzzle position, not from the rejected Bc4, and steps on.
    await expect.poll(() => pieceOn(page, 'c4')).toBe('')
    await expect.poll(() => pieceOn(page, 'f1')).toBe('white bishop')
    await page.keyboard.press('ArrowRight')
    await expect.poll(() => pieceOn(page, 'e4')).toBe('white pawn')
    expect(errors).toEqual([])
  })
})
