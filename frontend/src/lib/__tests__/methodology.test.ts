import { describe, expect, it } from 'vitest'
import analysisSource from '../../../../rust/src/analysis.rs?raw'
import { THRESHOLDS } from '../methodology'

// The Methodology page states the classification thresholds in prose; the backend applies them in
// rust/src/analysis.rs. Read the Rust source as text so a change on either side fails here. If a
// pattern stops matching after a refactor, update it to the new code rather than deleting the check.
function capture(pattern: RegExp): number[] {
  const match = analysisSource.match(pattern)
  if (!match) throw new Error(`Pattern not found in rust/src/analysis.rs: ${pattern}`)
  return match.slice(1).map(Number)
}

describe('methodology thresholds match the Rust analysis', () => {
  it('base classification limits', () => {
    const table = capture(
      /\((\d\.\d+), "excellent"\),\s*\((\d\.\d+), "good"\),\s*\((\d\.\d+), "inaccuracy"\),\s*\((\d\.\d+), "mistake"\),/,
    )
    expect(table).toEqual([THRESHOLDS.excellent, THRESHOLDS.good, THRESHOLDS.inaccuracy, THRESHOLDS.mistake])
  })

  it('brilliant', () => {
    expect(capture(/let undecided = ep_second\.is_some_and\(\|p\| p < (\d\.\d+)\);/)).toEqual([THRESHOLDS.brilliantMaxSecond])
    expect(capture(
      /if best\.depth >= (\d+)\s*&& second\.is_some_and\(\|s\| s\.depth == best\.depth\)\s*&& ep_played >= (\d\.\d+)/,
    )).toEqual([THRESHOLDS.minDepth, THRESHOLDS.brilliantMinScore])
    expect(capture(/let net = gain - captured;\s*if net < (\d+)/)).toEqual([THRESHOLDS.sacrificeMaterial])
  })

  it('great', () => {
    expect(capture(
      /&& full_best\.depth >= (\d+)\s*&& second\.is_some_and\(\|s\| s\.depth == full_best\.depth\)\s*&& ep_second\s*\.is_some_and\(\|s\|\s*\{?\s*(?:model\.)?expected_points\(full_best\.cp, white\) - s >= (\d\.\d+) && s < (\d\.\d+)\s*\}?\)\s*&& ep_played >= (\d\.\d+)/,
    )).toEqual([THRESHOLDS.minDepth, THRESHOLDS.greatGap, THRESHOLDS.greatMaxSecond, THRESHOLDS.greatMinScore])
  })

  it('miss', () => {
    expect(capture(
      /if previous_loss >= (\d\.\d+)\s*&& ep_best - previous_best >= (\d\.\d+)\s*&& ep_played >= previous_best - (\d\.\d+)/,
    )).toEqual([THRESHOLDS.missOpponentLoss, THRESHOLDS.missGain, THRESHOLDS.missKeep])
  })
})
