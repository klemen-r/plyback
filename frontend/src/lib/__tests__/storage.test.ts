import { afterEach, expect, it, vi } from 'vitest'
import { readStorage, storedChoice, writeStorage } from '../storage'

afterEach(() => vi.unstubAllGlobals())

it('keeps exact keys and values, validates choices and falls back for missing keys', () => {
  const values = new Map<string, string>()
  vi.stubGlobal('localStorage', { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) })
  writeStorage('ci.practiceLine', 'off')
  expect(readStorage('ci.practiceLine')).toBe('off')
  expect(readStorage('ci.absent')).toBeNull()
  expect(storedChoice('ci.practiceLine', ['on', 'off'], 'on')).toBe('off')
  expect(storedChoice('ci.practiceLine', ['on'], 'on')).toBe('on')
  expect(storedChoice('ci.absent', ['on', 'off'], 'on')).toBe('on')
  expect([...values]).toEqual([['ci.practiceLine', 'off']])
})

it('survives denied reads and writes', () => {
  vi.stubGlobal('localStorage', { getItem: () => { throw new Error('denied') }, setItem: () => { throw new Error('quota') } })
  expect(readStorage('ci.account')).toBeNull()
  expect(storedChoice('ci.practiceLine', ['on', 'off'], 'on')).toBe('on')
  expect(() => writeStorage('ci.account', '2')).not.toThrow()
})
