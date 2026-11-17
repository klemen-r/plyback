/** Guard the operation itself, including submissions made without a button click. */
export function singleFlight<Args extends unknown[], Result>(
  work: (...args: Args) => Promise<Result>,
  key: (...args: Args) => unknown = () => undefined,
  pending = new Set<unknown>(),
) {
  return async (...args: Args): Promise<Result | undefined> => {
    const id = key(...args)
    if (pending.has(id)) return undefined
    pending.add(id)
    try { return await work(...args) } finally { pending.delete(id) }
  }
}
