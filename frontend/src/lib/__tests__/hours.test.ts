import { afterAll, describe, expect, it } from 'vitest'
import { localHours } from '../hours'

// Vitest runs in Node; the app's tsconfig has no Node types.
declare const process: { env: Record<string, string | undefined> }

const original = process.env.TZ
afterAll(() => { process.env.TZ = original })

describe('localHours', () => {
  it('uses the local offset of each game’s own date, summer time included', () => {
    process.env.TZ = 'Europe/Ljubljana'
    const summer = Date.UTC(2025, 4, 29, 11, 56, 44) / 1000 // 13:56 in Ljubljana (UTC+2)
    const winter = Date.UTC(2025, 11, 1, 11, 56, 44) / 1000 // 12:56 in Ljubljana (UTC+1)
    const quarter = (t: number, n: number, points: number): [number, number, number] => [Math.floor(t / 900), n, points]
    const hours = localHours({ by_hour_utc: [], by_quarter_hour: [quarter(summer, 5, 4), quarter(winter, 6, 3)] })
    expect(hours[13]).toEqual({ hour: '13:00', games: 5, score: 80 })
    expect(hours[12]).toEqual({ hour: '12:00', games: 6, score: 50 })
    expect(hours[11].games).toBe(0)
  })

  it('hides hours with fewer than five games and falls back to UTC hours for old reports', () => {
    process.env.TZ = 'UTC'
    const hours = localHours({ by_hour_utc: [{ hour_utc: 9, games: 4, score: 50 }, { hour_utc: 10, games: 7, score: 64.3 }] })
    expect(hours[9]).toEqual({ hour: '9:00', games: 4, score: null })
    expect(hours[10]).toEqual({ hour: '10:00', games: 7, score: 64.3 })
  })
})
