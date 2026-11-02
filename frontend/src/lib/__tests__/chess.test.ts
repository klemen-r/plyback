import { describe, expect, it } from 'vitest'
import { legalDests, sanToSquares } from '../chess'

describe('board legality', () => {
  it('includes castling and en passant destinations', () => {
    const dests = legalDests('r3k2r/8/8/3pP3/8/8/8/R3K2R w KQkq d6 0 1')
    expect(dests.get('e1')).toEqual(expect.arrayContaining(['c1', 'g1']))
    expect(dests.get('e5')).toContain('d6')
  })
  it('does not offer moves leaving the king in check', () => {
    const dests = legalDests('4r1k1/8/8/8/8/8/8/R3K3 w Q - 0 1')
    expect(dests.has('a1')).toBe(false)
    expect(dests.get('e1')).not.toContain('c1')
  })
  it('offers promotion destinations once', () => {
    expect(legalDests('7k/P7/8/8/8/8/8/7K w - - 0 1').get('a7')).toEqual(['a8'])
  })
  it('rejects malformed positions and illegal notation without breaking evidence cards', () => {
    expect(legalDests('invalid').size).toBe(0)
    expect(sanToSquares('invalid', 'e4')).toBeNull()
    expect(sanToSquares('7k/8/8/8/8/8/8/7K w - - 0 1', 'e4')).toBeNull()
  })
})
