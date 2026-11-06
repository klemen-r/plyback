import { expect, test, type Page, type Route } from '@playwright/test'

const LEGAL = `[Event "E2E"]
[White "klemen_r"]
[Black "Friend"]
[Result "1-0"]

1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 Bxd1 6. Bxf7+ Ke7 7. Nd5# 1-0`

async function waitForAnalysis(page: Page, total: number) {
  await expect.poll(async () => {
    const r = await page.request.get('/api/accounts')
    const accs = await r.json()
    return accs[0]?.analyzed ?? 0
  }, { timeout: 150_000, intervals: [1000] }).toBe(total)
}

async function playOnBoard(page: Page, from: string, to: string) {
  // Clicks go to raw coordinates, so the whole board must be clear of the sticky header.
  await page.locator('cg-board').evaluate(el => el.scrollIntoView({ block: 'center' }))
  const board = await page.locator('cg-board').boundingBox()
  if (!board) throw new Error('Chessboard is not visible')
  for (const square of [from, to]) {
    await page.mouse.click(board.x + (square.charCodeAt(0) - 97 + 0.5) * board.width / 8,
      board.y + (8 - Number(square[1]) + 0.5) * board.height / 8)
  }
}

async function importPosition(page: Page, fen: string, move: string, variant = 'Standard') {
  await page.goto('/import')
  await page.getByLabel('PGN text').fill(`[White "Board test"]\n[Black "Opponent"]\n[Variant "${variant}"]\n[SetUp "1"]\n[FEN "${fen}"]\n[Result "*"]\n\n1. ${move} *`)
  await page.getByRole('switch', { name: 'Analyse immediately' }).click()
  await page.getByRole('button', { name: 'Import', exact: true }).click()
  await page.getByRole('link', { name: /Game #/ }).first().click()
  await expect(page.locator('cg-board')).toBeVisible()
}

test.describe.serial('Plyback end to end', () => {
  const errors = new WeakMap<Page, string[]>()
  test.beforeEach(async ({ page }) => {
    const messages: string[] = []
    errors.set(page, messages)
    page.on('pageerror', error => messages.push(error.message))
  })
  test.afterEach(async ({ page }) => expect(errors.get(page)).toEqual([]))
  test('onboarding: connect a Chess.com account', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Every game you play, reviewed.' })).toBeVisible()
    await page.getByLabel('Chess.com username').fill('no_such_player')
    await page.getByRole('button', { name: 'Connect' }).click()
    await expect(page.getByText(/No Chess.com player named/)).toBeVisible()
    await page.getByLabel('Chess.com username').fill('klemen_r')
    await page.getByRole('button', { name: 'Connect' }).click()
    await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible()
    await expect(page.getByText('Recent games')).toBeVisible()
    await expect(page.getByRole('row').nth(1)).toBeVisible({ timeout: 30_000 })
  })

  test('games are analysed automatically in the background', async ({ page }) => {
    await page.goto('/activity')
    await expect(page.getByRole('heading', { name: 'Activity' })).toBeVisible()
    await waitForAnalysis(page, 11)
    await page.goto('/games')
    await expect(page.getByText('11 games played by klemen_r')).toBeVisible()
    await page.getByLabel('Result').selectOption('loss')
    await expect(page).toHaveURL(/outcome=loss/)
    // How each game ended is listed, and can be filtered.
    await page.getByLabel('Result').selectOption('')
    await expect(page.getByRole('cell', { name: /^(Won|Lost) by checkmate$/ }).first()).toBeVisible()
    await page.getByLabel('Ending').selectOption('time')
    await expect(page).toHaveURL(/ending=time/)
    await expect(page.getByRole('cell', { name: /^(Won|Lost) on time$/ }).first()).toBeVisible()
    await expect(page.getByRole('cell', { name: /checkmate|resign/ })).toHaveCount(0)
  })

  test('game review: board, classifications, navigation, engine', async ({ page }) => {
    await page.goto('/games')
    await page.getByRole('row').nth(1).press('Enter')
    await expect(page.locator('cg-board')).toBeVisible()
    await expect(page.getByText('Accuracy').first()).toBeVisible()
    await expect(page.getByRole('img', { name: 'Evaluation graph' })).toBeVisible()
    // keyboard navigation reaches move 1 and shows its explanation card
    await page.keyboard.press('ArrowRight')
    await expect(page.getByText(/^1\. /).first()).toBeVisible()
    await page.keyboard.press('End')
    // The move list sits beside the current move; selecting a move updates the explanation.
    await page.locator('[data-ply="1"]').click()
    await expect(page.getByText(/^1\. /).first()).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Move quality' })).toBeVisible()
    await page.getByRole('button', { name: /Hint/ }).click()
    await expect(page.getByText('Live engine')).toBeVisible()
    // Desktop: the move controls and the side panel are reachable without scrolling the page.
    // Measure from the top of the page: the side panel is sticky, so its position moves with any scroll
    // that earlier clicks or key presses caused.
    const bottoms = await page.evaluate(() => {
      window.scrollTo(0, 0)
      return ['[role=toolbar]', 'main .grid.gap-6 > :nth-child(2)'].map(s => document.querySelector(s)!.getBoundingClientRect().bottom)
    })
    for (const bottom of bottoms) expect(bottom).toBeLessThanOrEqual(900)
    await page.screenshot({ path: test.info().outputPath('game-review-desktop.png') })
    await page.setViewportSize({ width: 390, height: 844 })
    // On phones the move controls are pinned to the bottom of the screen.
    const toolbar = page.getByRole('toolbar', { name: 'Move navigation' })
    await expect(toolbar).toHaveCSS('position', 'fixed')
    const box = await toolbar.boundingBox()
    expect(box && box.y + box.height).toBeGreaterThan(830)
    await toolbar.getByRole('button', { name: 'Next move' }).click()
    await expect(page.getByText(/^1… /).first()).toBeVisible()
    await page.screenshot({ path: test.info().outputPath('game-review-mobile.png') })
  })

  test('imported PGN: review explanations and mating position', async ({ page }) => {
    await page.goto('/import')
    await page.getByLabel('PGN text').fill(LEGAL)
    await page.getByRole('button', { name: 'Import' }).click()
    await expect(page.getByText(/1 new game/)).toBeVisible()
    await page.getByRole('link', { name: /Game #/ }).first().click()
    await expect(page.getByText(/Accuracy/).first()).toBeVisible({ timeout: 90_000 })
    await page.goto(page.url().split('?')[0] + '?ply=9')
    await expect(page.getByText('Engine lines before Nxe5')).toBeVisible({ timeout: 30_000 })
    await page.getByRole('button', { name: 'Last move' }).click()
    await expect(page.getByText('7. Nd5#', { exact: true })).toBeVisible()
  })

  test('practice: positions from your own mistakes, answers and scheduling', async ({ page }) => {
    await page.goto('/practice')
    await expect(page.getByRole('heading', { name: 'Practice' })).toBeVisible()
    await expect(page.getByText(/to move$/)).toBeVisible()
    // The answer is not in the page until an attempt.
    await expect(page.getByText(/The move was|Correct,/)).toHaveCount(0)
    const board = await page.locator('cg-board').boundingBox()
    expect(board!.y + board!.height).toBeLessThanOrEqual(900)
    await page.getByRole('button', { name: 'Show answer' }).click()
    await expect(page.getByText(/^The move was /)).toBeVisible()
    await expect(page.getByText(/comes back later in this session/)).toBeVisible()
    await page.getByRole('button', { name: 'Next move in line' }).click()
    await page.getByRole('link', { name: 'Open in game review' }).waitFor()
    await page.getByRole('button', { name: 'Next', exact: true }).click()
    await expect(page.getByText(/^2 \/ \d+$/)).toBeVisible()
    await page.getByRole('tab', { name: 'Blunders' }).click()
    await page.reload()
    await expect(page.getByRole('tab', { name: 'Blunders' })).toHaveAttribute('aria-selected', 'true')
    await page.getByRole('tab', { name: 'All' }).click()
    await expect(page.getByText('Learning')).toBeVisible()
    // The line toggle and the range are remembered; a range goes through answered positions too.
    const toggle = page.getByRole('switch', { name: 'Play out the line' })
    await expect(toggle).toHaveAttribute('aria-checked', 'true')
    await toggle.click()
    await page.getByLabel('Which games').selectOption('30')
    // Fixture games have fixed dates, so the range may be empty; either way the page says so.
    await expect(page.getByText(/positions from these games/).first()).toBeVisible()
    await page.reload()
    await expect(page.getByRole('switch', { name: 'Play out the line' })).toHaveAttribute('aria-checked', 'false')
    await expect(page.getByLabel('Which games')).toHaveValue('30')
  })

  test('openings: move tree by colour with results and navigation', async ({ page }) => {
    await page.goto('/openings')
    await expect(page.getByRole('heading', { name: 'Openings' })).toBeVisible()
    await expect(page.getByText(/reviewed games as White/)).toBeVisible()
    const first = page.locator('main button.grid').first()
    const san = (await first.locator('span').first().textContent())!.trim()
    await first.click()
    await expect(page).toHaveURL(/line=/)
    await expect(page.getByLabel('Current line')).toContainText(san)
    await expect(page.getByText(/games? reached this position/)).toBeVisible()
    await page.getByRole('button', { name: 'Back to the start' }).click()
    await expect(page.getByLabel('Current line')).toHaveText('Starting position')
    // Moves on the board follow the tree; a move no game played bounces back with a note.
    await playOnBoard(page, 'h2', 'h4')
    await expect(page.getByText('No game continued with h4 here.')).toBeVisible()
    await expect(page.getByLabel('Current line')).toHaveText('Starting position')
    // Switching colour never shows the other colour's tree, and a filter with no games says so.
    await page.getByRole('tab', { name: 'As Black' }).click()
    await expect(page).toHaveURL(/color=black/)
    await expect(page.getByText(/reviewed games as White/)).toHaveCount(0)
    await expect(page.getByText(/reviewed games as Black|No reviewed games as Black/)).toBeVisible()
    await page.getByLabel('Time control').selectOption('daily')
    await expect(page.getByText(/No reviewed daily games as Black|reviewed games as Black/)).toBeVisible()
  })

  test('insights dashboard with evidence and filters', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.getByRole('heading', { name: 'Account insights' })).toBeVisible()
    await expect(page.getByText(/games analysed/)).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Where to improve first' })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Openings', exact: true })).toBeVisible()
    await page.screenshot({ path: test.info().outputPath('insights.png') })
    await page.getByRole('button', { name: 'Blitz' }).click()
    await page.getByLabel('Colour', { exact: true }).selectOption('black')
    await expect(page.getByText(/games analysed/)).toBeVisible()
  })

  test('overview rating chart zooms, pans and resets', async ({ page }) => {
    await page.goto('/')
    const range = page.getByText(/^All \d+ games$/)
    await expect(range).toBeVisible()
    const total = Number((await range.textContent())!.match(/\d+/)![0])
    const chart = page.getByLabel(/^Chart: \+ and − zoom/)
    // Plain wheel scrolling is left to the page; Ctrl + wheel (and trackpad pinch) zooms.
    await chart.scrollIntoViewIfNeeded()
    const box = (await chart.boundingBox())!
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
    await page.keyboard.down('Control')
    await page.mouse.wheel(0, -400)
    await page.keyboard.up('Control')
    await expect(page.getByText(new RegExp(` · \\d+ of ${total} games$`))).toBeVisible()
    await expect(page.getByRole('scrollbar', { name: 'Visible range' })).toBeVisible()
    await page.getByRole('button', { name: 'Show everything' }).click()
    await expect(range).toBeVisible()
    await expect(page.getByRole('scrollbar', { name: 'Visible range' })).toHaveCount(0)
    // Buttons and keys do the same.
    await page.getByRole('button', { name: 'Zoom in' }).click()
    const scrollbar = page.getByRole('scrollbar', { name: 'Visible range' })
    await expect(scrollbar).toHaveAttribute('aria-valuenow', /\d+/)
    await chart.focus()
    for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowLeft')
    await expect(scrollbar).toHaveAttribute('aria-valuenow', '0')
    await page.keyboard.press('0')
    await expect(range).toBeVisible()
  })

  test('report: full account analysis snapshot', async ({ page }) => {
    await page.goto('/reports')
    await page.getByLabel('Report title').fill('E2E report')
    await page.getByRole('button', { name: /Analyse & create report/ }).click()
    await expect(page.getByRole('heading', { name: 'E2E report' })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Where to improve first' })).toBeVisible({ timeout: 60_000 })
  })

  test('compare and settings pages', async ({ page }) => {
    await page.goto('/compare')
    await expect(page.getByRole('heading', { name: 'Side by side', exact: true })).toBeVisible({ timeout: 30_000 })
    await page.goto('/settings')
    const workers = page.getByLabel('Parallel workers')
    const target = Number(await workers.inputValue()) === 1 ? 2 : 1
    await workers.fill(String(target))
    const save = page.getByRole('button', { name: /Save changes/ })
    await expect(save).toBeEnabled()
    await save.click()
    await expect.poll(async () => (await (await page.request.get('/api/settings')).json()).workers).toBe(target)
  })

  test('appearance: theme, any cover colour and board colours persist in this browser', async ({ page }) => {
    await page.goto('/settings')
    const accent = () => page.evaluate(() => document.documentElement.style.getPropertyValue('--accent'))
    await page.getByRole('button', { name: 'Brick' }).click()
    await expect(page.getByRole('button', { name: 'Brick' })).toHaveAttribute('aria-pressed', 'true')
    expect(await accent()).toBe('#b4402c')
    const hex = page.getByLabel('Cover colour hex')
    await hex.fill('#12')
    expect(await accent()).toBe('#b4402c')
    await hex.fill('#FFD000')
    expect(await accent()).toBe('#ffd000')
    await page.getByRole('tab', { name: 'Dark' }).click()
    await page.getByRole('tab', { name: /Walnut/ }).click()
    await page.reload()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
    expect(await accent()).toBe('#ffd000')
    await expect(page.getByRole('tab', { name: /Walnut/ })).toHaveAttribute('aria-selected', 'true')
    await page.getByRole('button', { name: 'Reset' }).click()
    await page.getByRole('tab', { name: 'System' }).click()
    expect(await accent()).toBe('#244b84')
  })

  test('mobile layout has working navigation without sideways scrolling', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/')
    await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Insights' }).click()
    await expect(page.getByRole('heading', { name: 'Account insights' })).toBeVisible()
    for (const path of ['/', '/insights', '/games', '/settings']) {
      await page.goto(path)
      await page.locator('main h1').first().waitFor()
      await page.waitForTimeout(1500)
      expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), path).toBeLessThanOrEqual(0)
    }
    await page.goto('/')
    await expect(page.getByRole('heading', { name: 'Latest game' })).toBeVisible()
    await expect(page.getByRole('link', { name: 'Open review' })).toBeVisible()
    await page.goto('/insights')
    await page.screenshot({ path: test.info().outputPath('mobile.png') })
  })

  test('review links reject malformed positions and search keeps keyboard focus', async ({ page }) => {
    await page.goto('/games')
    await page.getByRole('row').nth(1).press('Enter')
    const url = page.url().split('?')[0]
    for (const ply of ['-1', 'Infinity', '1.5', '999999']) {
      await page.goto(`${url}?ply=${ply}`)
      await expect(page.locator('cg-board')).toBeVisible()
    }
    const search = page.getByRole('button', { name: 'Search', exact: true })
    await search.click()
    const input = page.getByRole('combobox', { name: 'Search', exact: true })
    await expect(input).toBeFocused()
    await input.press('Tab')
    await expect(input).toBeFocused()
    await input.press('Escape')
    await expect(page.getByRole('dialog')).toHaveCount(0)
    await expect(search).toBeFocused()
  })

  test('board supports every promotion, castling and en passant; live requests stop and ignore superseded positions', async ({ page }) => {
    const pending: Route[] = []
    let failedRequests = 0
    page.on('requestfailed', request => { if (request.url().endsWith('/api/engine/analyse')) failedRequests++ })
    await page.route('**/api/engine/analyse', route => { pending.push(route) })
    await importPosition(page, '7k/P7/8/8/8/8/8/7K w - - 0 1', 'a8=Q+')
    await expect.poll(() => pending.length).toBe(1)
    await page.getByRole('button', { name: 'Stop engine' }).click()
    await expect(page.getByText('Engine paused.', { exact: true })).toBeVisible()
    await expect.poll(() => failedRequests).toBe(1)
    await page.getByRole('button', { name: 'Start engine' }).click()
    await expect.poll(() => pending.length).toBe(2)
    const fen = pending[1].request().postDataJSON().fen
    await pending[1].fulfill({ json: { fen, lines: [{ cp: 73, eval: '+0.73', win_white: 55, uci: [], san: [], depth: 20 }] } })
    await expect(page.getByText('+0.73', { exact: true })).toBeVisible()
    for (const [label, piece] of [['Queen', 'Q'], ['Rook', 'R'], ['Bishop', 'B'], ['Knight', 'N']]) {
      await page.getByRole('button', { name: 'First move' }).click()
      await playOnBoard(page, 'a7', 'a8')
      await page.getByRole('group', { name: 'Promote pawn' }).getByRole('button', { name: label, exact: true }).click()
      await expect(page.getByRole('link', { name: 'Lichess', exact: true })).toHaveAttribute('href', `https://lichess.org/analysis/${piece}6k/8/8/8/8/8/8/7K_b_-_-_0_1`)
    }
    await page.getByRole('button', { name: 'Stop engine' }).click()
    await expect(page.getByText('Engine paused.', { exact: true })).toBeVisible()

    await importPosition(page, 'r3k2r/8/8/3pP3/8/8/8/R3K2R w KQkq d6 0 1', 'O-O')
    await page.getByRole('button', { name: 'Stop engine' }).click()
    await playOnBoard(page, 'e1', 'g1')
    await expect(page.getByRole('link', { name: 'Lichess', exact: true })).toHaveAttribute('href', 'https://lichess.org/analysis/r3k2r/8/8/3pP3/8/8/8/R4RK1_b_kq_-_1_1')
    await page.getByRole('button', { name: 'First move' }).click()
    await playOnBoard(page, 'e5', 'd6')
    await expect(page.getByRole('link', { name: 'Lichess', exact: true })).toHaveAttribute('href', 'https://lichess.org/analysis/r3k2r/8/3P4/8/8/8/8/R3K2R_b_KQkq_-_0_1')
    await page.getByRole('button', { name: 'First move' }).click()
    await expect(page.getByText('+0.73', { exact: true })).toHaveCount(0)
  })

  test('Chess960 keeps recorded castling review and clearly limits unsupported exploration', async ({ page }) => {
    let liveRequests = 0
    page.on('request', request => { if (request.url().endsWith('/api/engine/analyse')) liveRequests++ })
    await importPosition(page, '4k3/8/8/8/8/8/8/6KR w K - 0 1', 'O-O', 'Chess960')
    await expect(page.getByText(/Chess960 games support recorded-move review/)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Hint', exact: true })).toBeDisabled()
    await page.getByRole('button', { name: 'Last move' }).click()
    await expect(page.getByRole('link', { name: 'Lichess', exact: true })).toHaveAttribute('href', 'https://lichess.org/analysis/chess960/4k3/8/8/8/8/8/8/5RK1_b_-_-_1_1')
    expect(liveRequests).toBe(0)
  })

  test('access password protects a new browser session and supports sign-in and sign-out', async ({ page, browser }) => {
    await page.goto('/settings')
    await page.getByLabel('New password', { exact: true }).fill('e2e-password-strong')
    await page.getByLabel('Confirm new password', { exact: true }).fill('e2e-password-strong')
    await page.getByRole('button', { name: 'Set access password' }).click()
    await expect(page.getByText('Access password updated.')).toBeVisible()
    const context = await browser.newContext()
    try {
      const guest = await context.newPage()
      await guest.goto(new URL('/', page.url()).href)
      await expect(guest.getByRole('heading', { name: 'Sign in to Plyback' })).toBeVisible()
      await guest.getByLabel('Password', { exact: true }).fill('wrong-password')
      await guest.getByRole('button', { name: 'Sign in', exact: true }).click()
      await expect(guest.getByRole('alert')).toContainText('not correct')
      await guest.getByLabel('Password', { exact: true }).fill('e2e-password-strong')
      await guest.getByRole('button', { name: 'Sign in', exact: true }).click()
      await expect(guest.getByRole('heading', { name: 'Overview' })).toBeVisible()
      await guest.getByRole('button', { name: 'Appearance and session' }).click()
      await guest.getByRole('menuitem', { name: 'Sign out' }).click()
      await expect(guest.getByRole('heading', { name: 'Sign in to Plyback' })).toBeVisible()
    } finally { await context.close() }
  })

  test('server connection errors offer a retry and keep account data behind the session gate', async ({ page }) => {
    let accountRequests = 0
    page.on('request', request => { if (request.url().endsWith('/api/accounts')) accountRequests++ })
    await page.route('**/api/auth/session', route => route.abort('failed'))
    await page.goto('/')
    await expect(page.getByRole('alert')).toContainText('not reachable')
    expect(accountRequests).toBe(0)
    await page.unroute('**/api/auth/session')
    await page.getByRole('button', { name: 'Retry connection' }).click()
    await expect(page.getByRole('heading', { name: 'Sign in to Plyback' })).toBeVisible()
  })
})
