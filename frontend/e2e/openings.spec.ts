import { expect, test } from '@playwright/test'
import type { Repertoire, RepertoireNode } from '../src/lib/types'
import { account as makeAccount, status as makeStatus, mockApi, playMove } from './support'

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'practice_fixture', display_name: 'Practice fixture', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 2, analyzed: 2 })], status: makeStatus({ games: 2, analyzed: 2, engine: { name: 'Fixture engine', error: null, path: null } }) }

  test('Openings depth preserves the line and distinguishes a depth boundary from missing games', async ({ page }) => {
    const sans = ['e4', 'e5', 'Nf3', 'Nc6', 'Bb5', 'a6', 'Ba4', 'Nf6', 'O-O', 'Be7', 'Re1', 'b5', 'Bb3', 'd6', 'c3', 'O-O', 'h3']
    const requestedDepths: number[] = []
    const { errors } = await mockApi(page, async (route, url) => {
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
    }, undefined, apiOptions)
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
    const { errors } = await mockApi(page, async (route, url) => {
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
    }, undefined, apiOptions)
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
})

test.describe(() => {
  test.use({ actionTimeout: 20_000 })
  const apiOptions = { accounts: [makeAccount({ username: 'review_fixture', display_name: 'Review fixture', sync_enabled: false, auto_analyze: false, analyze_history: false, games: 1 })], status: makeStatus({ games: 1, engine: { name: 'Fixture', error: null, path: null } }) }

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
    const { errors } = await mockApi(page, async (route, url) => {
      if (url.pathname === '/api/accounts/1/repertoire') await route.fulfill({ json: tree })
      else await route.fulfill({ json: [] })
    }, undefined, apiOptions)
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
})

test.describe(() => {
  const apiOptions = { accounts: [{ id: 1, username: 'fixture', display_name: 'Fixture', ratings: {}, games: 1, analyzed: 1, queued: 0, sync_status: 'idle' }], status: makeStatus({ games: 1, analyzed: 1, engine: { name: 'Fixture', error: null } }), emptyOpenings: true }

  test('Openings refreshes after a revision change even when account counts stay the same', async ({ page }) => {
    let revision = 1
    let reads = 0
    await mockApi(page, async route => {
      reads++
      await route.fulfill({ json: { color: 'white', plies: 16, total_games: 1, games: revision - 1,
        wins: revision - 1, draws: 0, losses: 0, nodes: [], positions: [], problems: [], merged: false, root: 0 } })
    }, () => revision, apiOptions)
    await page.clock.install()
    await page.goto('/openings')
    await expect(page.getByText('No reviewed games as White yet')).toBeVisible()
    revision = 2
    await page.clock.fastForward(16_000)
    await expect.poll(() => reads).toBe(2)
    await expect(page.getByText('No reviewed games as White yet')).toHaveCount(0)
  })
})

test.describe(() => {
  const apiOptions = { accounts: [makeAccount({ username: 'filters_fixture', display_name: 'Filters fixture' })], status: makeStatus(), handleFirst: true }

  test('Openings treats an unknown time control in the URL as all time controls', async ({ page }) => {
    const { seen } = await mockApi(page, undefined, undefined, apiOptions)
    await page.goto('/openings?tc=bogus')
    await expect(page.getByRole('combobox', { name: 'Time control' })).toHaveValue('all')
    await expect.poll(() => seen.some(u => u.pathname.endsWith('/repertoire'))).toBe(true)
    expect(seen.filter(u => u.pathname.endsWith('/repertoire')).every(u => !u.searchParams.has('time_class'))).toBe(true)
  })
})
