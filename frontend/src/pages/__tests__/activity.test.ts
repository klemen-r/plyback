import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router-dom'
import { expect, test } from 'vitest'
import type { QueueStats } from '../../lib/types'
import Activity from '../Activity'

const queue = {
  paused: false, workers: 1, engine: 'Fixture', engine_error: null, queued: 0, running: 0, failed: 0, done: 0,
  avg_game_seconds: null, eta_seconds: null, jobs: [], failed_jobs: [],
} as unknown as QueueStats

function render(client: QueryClient) {
  client.setQueryData(['queue'], queue)
  const html = renderToStaticMarkup(h(QueryClientProvider, { client }, h(MemoryRouter, null, h(Activity))))
  client.clear()
  return html
}

test('the log says it is loading, not empty, before the events arrive', () => {
  const html = render(new QueryClient())
  expect(html).toContain('Loading the log')
  expect(html).not.toContain('Nothing logged yet')
})

test('a failed log request shows the error, not an empty log', () => {
  // Without this the render would show the retry that follows the error.
  const client = new QueryClient({ defaultOptions: { queries: { retryOnMount: false } } })
  client.getQueryCache().build(client, { queryKey: ['events'] }).setState({ status: 'error', error: new Error('Events unavailable'), fetchStatus: 'idle' })
  const html = render(client)
  expect(html).toContain('Events unavailable')
  expect(html).not.toContain('Nothing logged yet')
})

test('an empty log still says so', () => {
  const client = new QueryClient()
  client.setQueryData(['events'], [])
  expect(render(client)).toContain('Nothing logged yet')
})
