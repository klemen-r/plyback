import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import { expect, test, vi } from 'vitest'
import { AccountCtx } from '../../lib/account-context'
import type { Account } from '../../lib/types'
import Openings from '../Openings'

vi.mock('../../components/Board', () => ({ default: () => null }))

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
