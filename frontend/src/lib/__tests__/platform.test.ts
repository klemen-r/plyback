import { describe, expect, it } from 'vitest'
import { linkedSite } from '../platform'

describe('linkedSite', () => {
  it('names the platform, or the host of an imported game link', () => {
    expect(linkedSite('lichess', 'https://lichess.org/abc')).toBe('Lichess')
    expect(linkedSite('pgn', 'https://www.chess.com/game/live/1')).toBe('Chess.com')
    expect(linkedSite('pgn', 'https://lichess.org/abcdefgh')).toBe('Lichess')
    expect(linkedSite('pgn', 'https://example.org/g/1')).toBe('example.org')
    expect(linkedSite('pgn', 'not a url')).toBe('the original site')
  })
})
