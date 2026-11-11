import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import { expect, test, vi } from 'vitest'
import { AccountCtx } from '../../lib/account-context'
import type { Account } from '../../lib/types'
import Openings from '../Openings'

vi.mock('../../components/Board', () => ({ default: () => null }))

test('"Open game" in Moves to fix opens the game at its own ply under merged move orders', () => {
  const client = new QueryClient()
  // 1. Nf3 d5 2. d4 reaches the d4 position at ply 3, but this game got there by 1. d4 d5 2. Nf3.
  const problem = { path: 'd4 d5 Nf3', san: 'Nf3', ply: 3, mine: true, from: 2, to: 3,
    games: 4, wins: 0, draws: 0, losses: 4, score: 0, orders: 2, book: 4, left_book: 0,
    errors: 3, blunders: 2, misses: 0, recent_games: [77], recent_plies: [5] }
  client.setQueryData(['repertoire', 1, 'white', 'all', 16, true], {
    merged: true, root: 0, games: 4, total_games: 4, wins: 0, draws: 0, losses: 4, plies: 16,
    nodes: [], problems: [problem], positions: [],
  })
  const account = { id: 1 } as Account
  const html = renderToStaticMarkup(
    h(QueryClientProvider, { client },
      h(AccountCtx.Provider, { value: { account, accounts: [account], loading: false, error: null, select: () => {} } },
        h(MemoryRouter, { initialEntries: ['/openings?merge=1'] }, h(Openings)))),
  )
  expect(html).toContain('href="/games/77?ply=5"')
  client.clear()
})

test('merged position subtitle uses distinct games and scores, not overlapping incoming edges', () => {
  const client = new QueryClient()
  const edge = { path: 'Nf3', san: 'Nf3', ply: 1, mine: true, from: 0, to: 1,
    games: 2, wins: 1, draws: 0, losses: 1, score: 0.5, orders: 1,
    book: 0, left_book: 0, errors: 0, recent_games: [], recent_plies: [] }
  client.setQueryData(['repertoire', 1, 'white', 'all', 16, true], {
    merged: true, root: 0, games: 2, total_games: 2, wins: 1, draws: 0, losses: 1,
    nodes: [edge, { ...edge, from: 2, games: 1, wins: 1, losses: 0 }], problems: [],
    positions: [{ id: 1, games: 2, wins: 1, draws: 0, losses: 1, score: 0.5, orders: 2 }],
  })
  const account = { id: 1 } as Account
  const html = renderToStaticMarkup(
    h(QueryClientProvider, { client },
      h(AccountCtx.Provider, { value: { account, accounts: [account], loading: false, error: null, select: () => {} } },
        h(MemoryRouter, { initialEntries: ['/openings?merge=1&line=Nf3'] }, h(Openings)))),
  )
  expect(html).toContain('2 of your games reached this position by 2 move orders')
  expect(html).toContain('you scored 50%')
  client.clear()
})

test('the subtitle leaves out the score when no reviewed game has a result', () => {
  const page = (root: { games: number; wins: number; draws: number; losses: number }) => {
    const client = new QueryClient()
    client.setQueryData(['repertoire', 1, 'black', 'all', 16, true], {
      merged: true, root: 0, total_games: root.games, plies: 16, nodes: [], problems: [], positions: [], ...root,
    })
    const account = { id: 1 } as Account
    const html = renderToStaticMarkup(
      h(QueryClientProvider, { client },
        h(AccountCtx.Provider, { value: { account, accounts: [account], loading: false, error: null, select: () => {} } },
          h(MemoryRouter, { initialEntries: ['/openings?color=black&merge=1'] }, h(Openings)))),
    )
    client.clear()
    return html
  }
  // Three unfinished PGN imports: games, but no wins, draws or losses.
  const unfinished = page({ games: 3, wins: 0, draws: 0, losses: 0 })
  expect(unfinished).toContain('3 reviewed games as Black')
  expect(unfinished).not.toContain('NaN')
  expect(unfinished).not.toContain('you scored')
  expect(page({ games: 4, wins: 1, draws: 2, losses: 0 })).toContain('you scored 67%')
})
