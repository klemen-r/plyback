import { afterEach, describe, expect, it, vi } from 'vitest'
import { api, ApiError, authEvents } from '../api'

afterEach(() => vi.unstubAllGlobals())

describe('API transport', () => {
  it('encodes user searches and sends same-origin credentials and CSRF protection', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] })))
    vi.stubGlobal('fetch', fetcher)
    await api.games({ q: 'name & other=?', analyzed: false })
    const [url, init] = fetcher.mock.calls[0]
    expect(new URL(url, 'http://localhost').searchParams.get('q')).toBe('name & other=?')
    expect(new URL(url, 'http://localhost').searchParams.get('analyzed')).toBe('false')
    expect(init.credentials).toBe('same-origin')
    expect(init.headers['X-Requested-With']).toBe('plyback')
  })

  it('reports unreachable servers as retryable network errors', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')))
    await expect(api.status()).rejects.toMatchObject({ status: 0 })
  })

  it('notifies the session gate when a session expires', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{"detail":"Sign in"}', { status: 401 })))
    const listener = vi.fn()
    authEvents.addEventListener('unauthorized', listener)
    try {
      await expect(api.accounts()).rejects.toEqual(new ApiError(401, 'Sign in'))
      expect(listener).toHaveBeenCalledOnce()
    } finally { authEvents.removeEventListener('unauthorized', listener) }
  })

  it('preserves server validation messages', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{"detail":[{"msg":"Invalid username"}]}', { status: 422 })))
    await expect(api.addAccount({ username: '?' })).rejects.toMatchObject({ status: 422, message: 'Invalid username' })
  })

  it('passes engine cancellation through without turning it into a connection error', async () => {
    const controller = new AbortController()
    const aborted = new DOMException('Aborted', 'AbortError')
    vi.stubGlobal('fetch', vi.fn().mockImplementation((_url, init) => {
      expect(init.signal).toBe(controller.signal)
      controller.abort()
      return Promise.reject(aborted)
    }))
    await expect(api.engine('position', 20, 3, controller.signal)).rejects.toBe(aborted)
  })
})
