import { readFile } from 'node:fs/promises'
import assert from 'node:assert/strict'
import test from 'node:test'
import { buildOpenings } from './build_openings.mjs'

test('opening generator reproduces the complete committed index from CC0 sources', async () => {
  const committed = JSON.parse(await readFile(new URL('../rust/data/openings.json', import.meta.url), 'utf8'))
  assert.deepEqual(await buildOpenings(), committed)
})
