import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseArgs, readme } from './package_release.mjs'

test('release arguments name a known target and a plain version', () => {
  const args = parseArgs(['--target', 'linux-arm64', '--version', '3.1.0', '--stockfish', 'sf', '--out', 'dist'])
  assert.deepEqual(args, { target: 'linux-arm64', version: '3.1.0', stockfish: 'sf', out: 'dist' })
  assert.throws(() => parseArgs(['--target', 'linux-riscv', '--version', '3.1.0', '--stockfish', 'sf', '--out', 'dist']), /Unknown target/)
  assert.throws(() => parseArgs(['--target', 'linux-x64', '--version', 'v3.1', '--stockfish', 'sf', '--out', 'dist']), /Version/)
  assert.throws(() => parseArgs(['--target', 'linux-x64', '--version', '3.1.0']), /--stockfish is required/)
  assert.throws(() => parseArgs(['--colour', 'blue']), /Unexpected argument/)
})

test('the bundled readme says how to start and where the data lives', () => {
  assert.match(readme('3.1.0', 'windows-x64'), /Double-click plyback\.exe/)
  assert.match(readme('3.1.0', 'windows-x64'), /plyback\.exe set-password/)
  assert.match(readme('3.1.0', 'linux-arm64'), /Run \.\/plyback/)
  assert.match(readme('3.1.0', 'linux-x64'), /copy data\/plyback\.db/)
  assert.match(readme('3.1.0', 'macos-arm64'), /xattr -dr com\.apple\.quarantine/)
  assert.doesNotMatch(readme('3.1.0', 'linux-x64'), /xattr/)
})
