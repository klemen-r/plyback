import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import GameReview from '../../pages/GameReview'

const { query } = vi.hoisted(() => ({ query: vi.fn() }))
vi.mock('@tanstack/react-query', () => ({
  useQuery: query,
  useQueryClient: () => ({ invalidateQueries: vi.fn(), cancelQueries: vi.fn() }),
  useMutation: () => ({ mutate: vi.fn() }),
}))
vi.mock('react-router-dom', () => ({
  useParams: () => ({ id: '42' }), useSearchParams: () => [new URLSearchParams('ply=1')], Link: 'a',
}))
vi.mock('../../components/Board', () => ({ default: () => null }))

beforeEach(() => query.mockReset())

describe('analyzed imported position move numbering', () => {
  it('uses FEN turns and fullmove numbers in key moments and move details', () => {
    const side = { username: 'Fixture', rating: null, result: null, accuracy: null, chesscom_accuracy: null }
    // Answer by query key, not call order: the review also reads Settings and the live engine.
    const game = {
      game: { id: 42, white: side, black: side, rules: 'chess', result: '*', opening: {}, job: null,
        initial_fen: '7k/8/8/8/8/8/8/7K b - - 0 42', final_fen: '8/7k/8/8/8/8/7K/8 b - - 2 43' },
      analysis: { accuracy: {}, acpl: {}, counts: { white: {}, black: {} }, opening: {},
        key_moments: [1, 2], evals: [0, 0, 0], win_white: [50, 50, 50], depth: 12 },
      moves: [
        { ply: 1, color: 'b', san: 'Kh7', uci: 'h8h7', classification: 'mistake', fen_before: '7k/8/8/8/8/8/8/7K b - - 0 42' },
        { ply: 2, color: 'w', san: 'Kh2', uci: 'h1h2', classification: 'mistake', fen_before: '8/7k/8/8/8/8/8/7K w - - 1 43' },
      ],
    }
    query.mockImplementation((options?: { queryKey?: unknown[] }) => {
      const key = options?.queryKey?.[0]
      return key === 'game' ? { data: game }
        : key === 'settings' ? { data: { interactive_depth: 20 }, isPending: false }
          : { data: { lines: [] } }
    })
    const html = renderToStaticMarkup(createElement(GameReview))
    expect(html).toContain('>42… Kh7</span>')
    expect(html).toContain('>43. Kh2</span>')
    expect(html).not.toContain('>1. Kh7</span>')
    expect(html).not.toContain('>1… Kh2</span>')
    // Selected move detail and its key-moment button both keep Black's turn.
    expect(html.match(/>42… Kh7<\/span>/g)).toHaveLength(2)
  })
})
