import type {
  Account, EventRow, GameDetail, GamesPage, InsightFilters, Insights, InteractiveResult, PracticeKind, PracticePhase, PracticeQueue, PracticeResult, QueueDetail, Repertoire, Report, Session, Settings, Status,
} from './types'

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

/** Raised for 401s so the app can switch to the sign-in screen. */
export const authEvents = new EventTarget()

async function request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  let res: Response
  try {
    res = await fetch(`/api${path}`, {
      method,
      signal,
      credentials: 'same-origin',
      headers: {
        'X-Requested-With': 'plyback',
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
  } catch (error) {
    if (signal?.aborted) throw error
    throw new ApiError(0, 'Plyback’s server is not reachable. Is it still running?')
  }
  if (!res.ok) {
    let msg = `Request failed (${res.status})`
    try {
      const data = await res.json()
      if (typeof data.detail === 'string') msg = data.detail
      else if (Array.isArray(data.detail)) msg = data.detail.map((d: { msg: string }) => d.msg).join('; ')
    } catch { /* not JSON */ }
    if (res.status === 401) authEvents.dispatchEvent(new Event('unauthorized'))
    throw new ApiError(res.status, msg)
  }
  return (res.status === 204 ? undefined : res.json()) as Promise<T>
}

function qs(params: object): string {
  const sp = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') continue
    sp.set(k, Array.isArray(v) ? v.join(',') : String(v))
  }
  const s = sp.toString()
  return s ? `?${s}` : ''
}

export interface GameQuery {
  account_id?: number; page?: number; page_size?: number; time_class?: string; outcome?: string; color?: string
  opening?: string; analyzed?: boolean; q?: string; since?: number; until?: number; rated?: boolean
  classification?: string; sort?: string
}

export const api = {
  session: () => request<Session>('GET', '/auth/session'),
  login: (password: string) => request<Session>('POST', '/auth/login', { password }),
  logout: () => request<{ ok: boolean }>('POST', '/auth/logout'),
  setPassword: (current: string | null, password: string | null) => request<Session>('POST', '/auth/password', { current, password }),

  status: () => request<Status>('GET', '/status'),
  settings: () => request<Settings>('GET', '/settings'),
  saveSettings: (patch: Partial<Settings>) => request<Settings>('PUT', '/settings', patch),
  events: (limit = 100, account_id?: number) => request<EventRow[]>('GET', `/events${qs({ limit, account_id })}`),

  accounts: () => request<Account[]>('GET', '/accounts'),
  addAccount: (body: { username: string; import_since?: string | null; analyze_history?: boolean; auto_analyze?: boolean }) =>
    request<Account>('POST', '/accounts', body),
  patchAccount: (id: number, body: Partial<Pick<Account, 'sync_enabled' | 'auto_analyze' | 'analyze_history' | 'import_since'>>) =>
    request<Account>('PATCH', `/accounts/${id}`, body),
  deleteAccount: (id: number) => request<{ deleted: boolean; games_removed: number }>('DELETE', `/accounts/${id}`),
  syncAccount: (id: number) => request<{ requested: boolean }>('POST', `/accounts/${id}/sync`),
  analyzeScope: (id: number, filters: InsightFilters, depth?: number) =>
    request<{ scope: number; queued: number }>('POST', `/accounts/${id}/analyze`, { filters, depth }),

  games: (q: GameQuery) => request<GamesPage>('GET', `/games${qs(q)}`),
  openings: (account_id?: number) => request<{ family: string; n: number }[]>('GET', `/games/openings${qs({ account_id })}`),
  game: (id: number, account_id?: number) => request<GameDetail>('GET', `/games/${id}${qs({ account_id })}`),
  analyzeGame: (id: number, depth?: number, force = false) => request<{ queued: boolean }>('POST', `/games/${id}/analyze`, { depth, force }),
  importPgn: (pgn: string, analyze = true) =>
    request<{ imported: number; existing: number; game_ids: number[]; errors: { index: number; error: string }[] }>('POST', '/games/import', { pgn, analyze }),

  engine: (fen: string, depth?: number, multipv = 3, signal?: AbortSignal) => request<InteractiveResult>('POST', '/engine/analyse', { fen, depth, multipv }, signal),

  queue: () => request<QueueDetail>('GET', '/queue'),
  queueAction: (action: 'pause' | 'resume' | 'retry-failed' | 'clear') => request<Record<string, unknown>>('POST', `/queue/${action}`),

  insights: (account_id: number, f: InsightFilters) => request<Insights>('GET', `/accounts/${account_id}/insights${qs(f)}`),
  reports: (account_id: number) => request<Report[]>('GET', `/accounts/${account_id}/reports`),
  createReport: (account_id: number, filters: InsightFilters, title?: string) =>
    request<Report>('POST', `/accounts/${account_id}/reports`, { filters, title }),
  report: (id: number) => request<Report>('GET', `/reports/${id}`),

  practice: (account_id: number, kind?: PracticeKind, limit = 10, since?: number, phase?: PracticePhase) =>
    request<PracticeQueue>('GET', `/accounts/${account_id}/practice${qs({ kind, limit, since, phase })}`),
  repertoire: (account_id: number, q: { color: 'white' | 'black'; time_class?: string; plies?: number; merge?: boolean }) =>
    request<Repertoire>('GET', `/accounts/${account_id}/repertoire${qs(q)}`),
  answerPractice: (account_id: number, game_id: number, ply: number, answer: { uci: string } | { reveal: true }) =>
    request<PracticeResult>('POST', `/accounts/${account_id}/practice/${game_id}/${ply}`, answer),
  finalizeReport: (id: number) => request<Report>('POST', `/reports/${id}/finalize`),
  deleteReport: (id: number) => request<{ deleted: boolean }>('DELETE', `/reports/${id}`),
}
