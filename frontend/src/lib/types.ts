export type Classification =
  | 'brilliant' | 'great' | 'best' | 'excellent' | 'good' | 'book' | 'forced'
  | 'inaccuracy' | 'mistake' | 'miss' | 'blunder'

export type Color = 'white' | 'black'
export type Confidence = 'insufficient' | 'low' | 'moderate' | 'high'

export interface Account {
  id: number
  username: string
  display_name: string
  avatar: string | null
  title: string | null
  country: string | null
  profile_url: string
  joined: number | null
  ratings: Record<string, { rating: number | null; best: number | null; record?: { win: number; loss: number; draw: number } }>
  added_at: number
  last_sync_at: number | null
  sync_status: 'idle' | 'syncing' | 'error'
  sync_error: string | null
  sync_enabled: boolean
  auto_analyze: boolean
  analyze_history: boolean
  import_since: string | null
  games: number
  analyzed: number
  queued: number
}

export interface PlayerSide {
  username: string | null
  rating: number | null
  result: string | null
  accuracy: number | null
  chesscom_accuracy: number | null
}

export interface Counts { white: Partial<Record<Classification, number>>; black: Partial<Record<Classification, number>> }

export interface JobInfo { status: 'queued' | 'running' | 'failed'; error?: string; progress?: number; total?: number }

export interface GameSummary {
  id: number
  url: string | null
  source: string
  end_time: number | null
  time_class: string | null
  time_control: string | null
  rated: boolean | null
  rules: string
  result: string
  termination: string | null
  plies: number | null
  white: PlayerSide
  black: PlayerSide
  opening: { eco: string | null; name: string | null; family: string | null }
  user_color: Color | null
  outcome: 'win' | 'loss' | 'draw' | null
  analyzed: boolean
  analysis_depth: number | null
  counts: Counts | null
  job: JobInfo | null
  pgn?: string
  initial_fen?: string
  final_fen?: string
  pgn_error?: string
}

export interface EngineLine { uci: string | null; cp: number; eval: string; san: string[]; depth: number }

export interface MoveRow {
  ply: number
  color: 'w' | 'b'
  san: string
  uci: string
  fen_before: string
  eval_before?: number
  eval_after?: number
  win_before?: number
  win_after?: number
  ep_loss?: number
  cp_loss?: number
  accuracy?: number
  best_uci?: string | null
  best_san?: string | null
  classification?: Classification
  phase?: string
  clock?: number | null
  time_spent?: number | null
  is_book?: boolean
  tags?: Record<string, Record<string, unknown>>
  explanation?: string
  lines?: EngineLine[]
  reply?: { uci: string | null; san: string[]; cp: number } | null
}

export interface GameAnalysis {
  depth: number
  multipv: number
  engine: string
  version: number
  outdated: boolean
  analyzed_at: number
  duration_ms: number | null
  accuracy: { white: number | null; black: number | null }
  acpl: { white: number | null; black: number | null }
  counts: Counts
  opening: { eco: string | null; name: string | null; family: string | null; variation: string | null }
  book_plies: number
  key_moments: number[]
  evals: number[]
  win_white: number[]
}

export interface GameDetail { game: GameSummary; analysis: GameAnalysis | null; moves: MoveRow[] }

export interface GamesPage { total: number; page: number; page_size: number; items: GameSummary[] }

export interface QueueStats {
  paused: boolean
  workers: number
  engine: string | null
  engine_error: string | null
  queued: number
  running: number
  failed: number
  done: number
  active: { job_id: number; game_id: number; done: number; total: number; worker: number }[]
  avg_game_seconds: number | null
  eta_seconds: number | null
}

export interface Status {
  version: string
  analysis_version: number
  data_revision?: number
  engine: { name: string | null; path: string | null; error: string | null }
  queue: QueueStats
  sync: { current_account: number | null }
  accounts: number
  games: number
  analyzed: number
  db_size: number | null
  depth_presets: Record<string, number>
  cpu_count: number
}

export interface Settings {
  analysis_depth: number
  analysis_multipv: number
  max_seconds_per_position: number
  workers: number
  threads_per_worker: number
  hash_mb: number
  interactive_depth: number
  sync_interval_seconds: number
  active_sync_interval_seconds: number
  auto_analyze_new: boolean
  analyze_history: boolean
  contact: string
}

export interface InsightFilters {
  since?: number
  until?: number
  time_classes?: string[]
  color?: Color
  rated?: boolean
  opening?: string
  opponent?: string
}

export interface WDL {
  games: number; wins: number; draws: number; losses: number
  score: number | null; score_ci: [number, number]; confidence: Confidence
}

export interface Example {
  game_id: number; ply: number; fen: string | null; san: string; best_san: string | null
  classification: Classification; ep_loss: number; explanation: string | null; phase: string
  opponent: string; end_time: number; url: string | null; color: Color; outcome: string
  time_class: string | null; lichess_url: string | null
}

export interface Resource { title: string; url: string; kind: string }

export interface Rates {
  moves: number; accuracy: number | null; acpl: number | null
  inaccuracies_per_100: number | null; mistakes_per_100: number | null; blunders_per_100: number | null
}

export interface OpeningFamily extends WDL {
  family: string
  accuracy: number | null
  opening_accuracy: number | null
  opening_errors_per_game: number | null
  exit_win: number | null
  analyzed: number
  flag: 'weakness' | 'strength' | null
  variations: (WDL & { name: string })[]
  recent_game_ids: number[]
  resources: Resource[]
}

export interface Priority {
  key: string; title: string; rank: number | null; score: number
  ep_lost_per_game?: number; opp_ep_lost_per_game?: number; vs_opponents?: number
  errors?: number; errors_per_game?: number; opp_errors_per_game?: number
  confidence: Confidence; summary: string; detail?: string
  examples: Example[]; resources: Resource[]
}

export interface Insights {
  filters: InsightFilters
  generated_at?: number
  stale?: boolean
  data_quality: { games: number; analyzed: number; coverage: number | null; unanalyzed_ids?: number[]; notes: string[]; confidence: Confidence }
  overview: WDL & {
    white: WDL; black: WDL
    by_time_class: (WDL & { time_class: string; accuracy: number | null })[]
    analyzed: number; accuracy: number | null; opp_accuracy: number | null; acpl: number | null
    chesscom_accuracy: number | null; chesscom_accuracy_games: number
    performance_rating: number | null; avg_opponent_rating: number | null
    per_game: Record<Classification, number | null>; opp_per_game: Record<Classification, number | null>
    first_game: number | null; last_game: number | null
  }
  ratings: Record<string, { points: { t: number; rating: number; game_id: number }[]; start: number; end: number; change: number; peak: number; low: number; games: number }>
  results: {
    wins_by: Record<string, number>; losses_by: Record<string, number>; draws_by: Record<string, number>
    by_opponent: (WDL & { label: string; expected: number })[]
    by_hour_utc: { hour_utc: number; games: number; score: number | null }[]
    /** 15-minute UTC buckets: [index (unix time / 900), games, score points]. Absent in older saved reports. */
    by_quarter_hour?: [number, number, number][]
    by_length: (WDL & { label: string })[]
  }
  sessions: {
    sessions: number
    after: Record<'win' | 'loss' | 'draw', { games: number; score: number | null; confidence: Confidence }>
    by_game_in_session: Record<string, { games: number; score: number | null }>
    tilt: { score_after_loss: number; baseline: number; difference: number; significant: boolean } | null
  }
  trend: { period: string; games: number; score: number | null; accuracy: number | null; analyzed: number; blunders_per_game: number | null; rating: number | null }[]
  openings: Record<Color, { families: OpeningFamily[]; first_moves: (WDL & { move: string })[] }>
  phases: Record<'opening' | 'middlegame' | 'endgame', { user: Rates; opponent: Rates; games: number; ep_lost_per_game: number; opp_ep_lost_per_game: number; confidence: Confidence }> & {
    endgame_types: { type: string; games: number; user: Rates; opponent: Rates; confidence: Confidence }[]
  }
  errors: {
    tags: { tag: string; label: string; count: number; per_game: number; opp_per_game: number; ep_lost_per_game: number; examples: Example[]; confidence: Confidence }[]
    hung_pieces: Record<string, number>
    total_errors: number; tactical_share: number | null; positional_share: number | null
    errors_per_game: number; opp_errors_per_game: number
  }
  time: {
    games: number; confidence: Confidence
    avg_time_per_move?: Record<string, { user: number | null; opponent: number | null }>
    clock_curve?: { move: number; user: number; opponent: number | null; games: number }[]
    time_trouble?: { games: number; share: number | null; score: number | null; errors_per_100_low: number | null; errors_per_100_normal: number | null; moves_low: number }
    speed_vs_quality?: { label: string; range: string; moves: number; errors_per_100: number | null; blunders_per_100: number | null; accuracy: number | null }[]
    timeouts?: { losses_on_time: number; share_of_losses: number | null; in_non_losing_position: number; analyzed: number; wins_on_time: number; examples: { game_id: number; opponent: string; end_time: number }[] }
    clock_left_at_end?: Record<string, number | null>
  }
  advantage: {
    winning_positions: number; converted: number; conversion_rate: number | null; conversion_confidence: Confidence
    losing_positions: number; saved: number; save_rate: number | null
    lost_advantages: { game_id: number; opponent: string; end_time: number; peak_win: number; outcome: string; reason: string; culprit: Example | null }[]
    how_you_lose: { key: string; phase: string; kind: string; count: number; share: number | null; examples: Example[] }[]
    losses_analyzed: number
  }
  missed: {
    opponent_errors: number; punished: number; punish_rate: number | null; confidence: Confidence
    missed_count: number; missed_mates: number; examples: Example[]; missed_mate_examples: Example[]
  }
  recurring: {
    fen: string; occurrences: number; games: number[]; played: { san: string; count: number }[]
    best_san: string | null; avg_ep_loss: number; phase: string; move_number: number; color: Color
    explanation: string | null; example: Example; lichess_url: string | null; impact: number
  }[]
  priorities: Priority[]
}

export interface Report {
  id: number
  account_id: number
  title: string | null
  status: 'analyzing' | 'ready' | 'failed'
  total_games: number
  analyzed_games: number
  created_at: number
  completed_at: number | null
  error: string | null
  filters: InsightFilters
  report?: Insights | null
}

export interface EventRow { id: number; ts: number; level: string; source: string; message: string; account_id: number | null }

export interface QueueDetail extends QueueStats {
  jobs: { id: number; game_id: number; priority: number; depth: number; status: string; attempts: number; created_at: number; white_username: string; black_username: string; end_time: number; time_class: string; progress?: number; total?: number }[]
  failed_jobs: { id: number; game_id: number; error: string; attempts: number; finished_at: number; white_username: string; black_username: string }[]
}

export interface InteractiveResult { fen: string; lines: { cp: number; eval: string; win_white: number; uci: string[]; san: string[]; depth: number }[] }

export interface Session {
  authenticated: boolean
  password_set: boolean
  auth_required: boolean
}

export type PracticeKind = 'blunder' | 'mistake' | 'miss'
export type PracticePhase = 'opening' | 'middlegame' | 'endgame'
export type PracticeTheme = 'mate-in-1' | 'mate-in-2' | 'mate-in-3-plus' | 'back-rank-mate' | 'fork' | 'knight-fork'
  | 'pin' | 'skewer' | 'discovered-attack' | 'hanging-piece' | 'trapped-piece' | 'promotion' | 'defensive'
export interface PracticeStats { total: number; new: number; due: number; learning: number; mastered: number; next_due_at: number | null }
export interface PracticeItem {
  game_id: number; ply: number; fen: string; color: Color; move_number: number
  classification: PracticeKind; phase: string | null; ep_loss: number; themes: PracticeTheme[]
  last_uci: string | null; last_san: string | null; end_time: number | null; time_class: string | null
  opponent: string | null; opponent_rating: number | null
  status: 'new' | 'due' | 'scheduled'; box: number; attempts: number; correct: number
}
export interface PracticeQueue { stats: PracticeStats; items: PracticeItem[]; theme_counts: Partial<Record<PracticeTheme, number>> }
export interface PracticeResult {
  correct: boolean; uci: string | null; san: string | null; best_uci: string; best_san: string
  themes: PracticeTheme[]; line_san: string[]; game_san: string; explanation: string | null; box: number; due_at: number; stats: PracticeStats
}

export interface RepertoireNode {
  path: string; san: string; ply: number; mine: boolean
  games: number; wins: number; draws: number; losses: number; score: number; last_played: number
  book: number; left_book: number; errors: number | null; blunders: number | null; misses: number | null; avg_loss: number | null
  eco: string | null; opening: string | null; recent_games: number[]; recent_plies: number[]
  /** Position ids before and after the move; `orders` is how many move orders reached it. */
  from: number; to: number; orders: number
}
export interface Repertoire {
  color: Color; plies: number; merged: boolean; root: number; games: number; total_games: number; wins: number; draws: number; losses: number
  nodes: RepertoireNode[]; problems: RepertoireNode[]
}
