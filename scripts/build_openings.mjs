// Regenerate the CC0 Lichess opening index using the locked frontend chess.js.
import { createRequire } from 'node:module'
import { readFile, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

const require = createRequire(new URL('../frontend/package.json', import.meta.url))
const { Chess } = require('chess.js')
const data = new URL('../rust/data/', import.meta.url)

export async function buildOpenings() {
  const named = new Map()
  const book = new Set()
  for (const letter of 'abcde') {
    const text = await readFile(new URL(`openings/${letter}.tsv`, data), 'utf8')
    for (const row of text.trimEnd().split(/\r?\n/).slice(1)) {
      const [eco, name, pgn] = row.split('\t')
      const game = new Chess()
      game.loadPgn(pgn)
      const moves = game.history({ verbose: true })
      for (const move of moves) book.add(move.after.split(' ').slice(0, 4).join(' '))
      const epd = game.fen().split(' ').slice(0, 4).join(' ')
      const existing = named.get(epd)
      if (!existing || moves.length < existing[2]) named.set(epd, [eco, name, moves.length])
    }
  }
  return { named: Object.fromEntries([...named].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)), book: [...book].sort() }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const output = new URL('openings.json', data)
  await writeFile(output, JSON.stringify(await buildOpenings()))
  console.log(`Wrote ${fileURLToPath(output)}`)
}
