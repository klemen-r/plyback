import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import MoveList from '../../components/MoveList'
import type { MoveRow } from '../types'

function renderMoves(moves: MoveRow[]) {
  return renderToStaticMarkup(createElement(MoveList, { moves, current: 0, onSelect: () => {} }))
}

describe('imported position move numbering', () => {
  it('uses the FEN fullmove number for a white promotion', () => {
    const html = renderMoves([{ ply: 1, color: 'w', san: 'a8=Q+', uci: 'a7a8q', fen_before: '7k/P7/8/8/8/8/8/7K w - - 0 42' }])
    expect(html).toContain('>42.</span>')
    expect(html).not.toContain('>1.</span>')
  })

  it('keeps a black starting move in its own fullmove row', () => {
    const html = renderMoves([
      { ply: 1, color: 'b', san: 'Kh7', uci: 'h8h7', fen_before: '7k/8/8/8/8/8/8/7K b - - 0 42' },
      { ply: 2, color: 'w', san: 'Kh2', uci: 'h1h2', fen_before: '8/7k/8/8/8/8/8/7K w - - 1 43' },
    ])
    expect(html).toContain('>42.</span><span></span><button data-ply="1"')
    expect(html).toContain('>43.</span><button data-ply="2"')
  })
})
