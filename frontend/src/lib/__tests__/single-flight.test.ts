import { expect, it } from 'vitest'
import { singleFlight } from '../single-flight'

it('ignores overlapping submissions and allows a later submission', async () => {
  let resolve!: (value: number) => void
  let calls = 0
  const run = singleFlight((value: number) => { calls++; return new Promise<number>(r => { resolve = r }).then(n => n + value) })
  const first = run(3)
  expect(await run(4)).toBeUndefined()
  expect(calls).toBe(1)
  resolve(2)
  expect(await first).toBe(5)
  const later = run(7)
  resolve(1)
  expect(await later).toBe(8)
  expect(calls).toBe(2)
})

it('releases the guard after rejection and synchronous throws', async () => {
  let fail = true
  const run = singleFlight(() => { if (fail) throw new Error('failed'); return Promise.resolve('ok') })
  await expect(run()).rejects.toThrow('failed')
  fail = false
  expect(await run()).toBe('ok')
})

it('keeps independent report operations available while rejecting a duplicate row submission', async () => {
  const resolvers = new Map<number, () => void>()
  const run = singleFlight((id: number) => new Promise<void>(resolve => resolvers.set(id, resolve)), id => id)
  const first = run(1), second = run(2)
  expect(await run(1)).toBeUndefined()
  expect([...resolvers.keys()]).toEqual([1, 2])
  resolvers.get(1)!(); resolvers.get(2)!()
  await Promise.all([first, second])
})
