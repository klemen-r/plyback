/**
 * Classification thresholds shown on the Methodology page, as fractions of expected points
 * (0.05 = 5 percentage points) or engine depths and material points. They mirror
 * `rust/src/analysis.rs`; `__tests__/methodology.test.ts` fails when the two differ.
 */
export const THRESHOLDS = {
  excellent: 0.02,
  good: 0.05,
  inaccuracy: 0.1,
  mistake: 0.2,
  /** Brilliant and Great need best and second-best lines at this completed depth or more. */
  minDepth: 10,
  sacrificeMaterial: 2,
  brilliantMinScore: 0.5,
  brilliantMaxSecond: 0.8,
  greatGap: 0.15,
  greatMinScore: 0.35,
  greatMaxSecond: 0.9,
  missOpponentLoss: 0.1,
  missGain: 0.1,
  missKeep: 0.05,
} as const

/** A fraction of expected points as whole percentage points. */
export function points(fraction: number): number {
  return Math.round(fraction * 100)
}
