import { useState } from 'react'
import { singleFlight } from './single-flight'

/** Mutation errors are rendered at their submit site; prevent overlapping submissions. */
export function useSingleFlight<Args extends unknown[], Result>(work: (...args: Args) => Promise<Result>, key?: (...args: Args) => unknown): (...args: Args) => void {
  // Keep the lock across renders while each handler uses the current mutation and arguments.
  const [pending] = useState(() => new Set<unknown>())
  const guarded = singleFlight(work, key, pending)
  return (...args: Args) => { void guarded(...args).catch(() => { /* Mutation owns its error state. */ }) }
}
