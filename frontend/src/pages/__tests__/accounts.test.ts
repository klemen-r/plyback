import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import Accounts from '../Accounts'
import { AccountCtx } from '../../lib/account-context'
import type { Account } from '../../lib/types'

const { mutation } = vi.hoisted(() => ({ mutation: vi.fn() }))
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
  useMutation: mutation,
}))
vi.mock('react-router-dom', () => ({ useNavigate: () => vi.fn() }))
vi.mock('../../components/Layout', () => ({ Avatar: () => null }))

const account = (id: number): Account => ({
  id, username: `fixture${id}`, display_name: `Fixture ${id}`, avatar: null, title: null,
  country: null, profile_url: '', joined: null, ratings: {}, added_at: 0, last_sync_at: null,
  sync_status: 'idle', sync_error: null, sync_enabled: true, auto_analyze: true,
  analyze_history: true, import_since: null, games: 0, analyzed: 0, queued: 0, platform: 'chesscom',
})

beforeEach(() => mutation.mockReset())

describe('account mutation errors', () => {
  it.each(['patch', 'sync', 'delete'])('shows a failed %s on its own account card', action => {
    const state = { mutate: vi.fn(), isPending: false, error: null }
    // Connect form, then patch/sync/delete for each of the two account cards.
    const errorIndex = { patch: 1, sync: 2, delete: 3 }[action]!
    let index = 0
    mutation.mockImplementation(() => ({ ...state, error: index++ === errorIndex ? new Error(`${action} unavailable`) : null }))
    const accounts = [account(1), account(2)]
    const html = renderToStaticMarkup(createElement(AccountCtx.Provider, {
      value: { accounts, account: accounts[0], loading: false, error: null, select: vi.fn() },
    }, createElement(Accounts)))
    expect(html).toContain('role="alert"')
    expect(html.indexOf(`${action} unavailable`)).toBeGreaterThan(html.indexOf('Fixture 1'))
    expect(html.indexOf(`${action} unavailable`)).toBeLessThan(html.indexOf('Fixture 2'))
    expect(html.match(/role="alert"/g)).toHaveLength(1)
  })
})
