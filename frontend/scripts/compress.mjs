// Precompress public build assets once, keeping runtime CPU available for review.
import { readdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { brotliCompress, constants, gzip } from 'node:zlib'

const brotli = promisify(brotliCompress)
const gz = promisify(gzip)
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
let identityBytes = 0
let brotliBytes = 0
let gzipBytes = 0
for (const name of await readdir(dist, { recursive: true })) {
  if (!/\.(?:html|js|css|svg|json)$/.test(name)) continue
  const file = path.join(dist, name)
  const source = await readFile(file)
  if (source.length < 1024) continue
  const [br, compressed] = await Promise.all([
    brotli(source, { params: { [constants.BROTLI_PARAM_QUALITY]: 9 } }),
    gz(source, { level: 9 }),
  ])
  if (Math.max(br.length, compressed.length) >= source.length * 0.9) continue
  await Promise.all([writeFile(`${file}.br`, br), writeFile(`${file}.gz`, compressed)])
  identityBytes += source.length
  brotliBytes += br.length
  gzipBytes += compressed.length
}
console.log(`Static assets: ${identityBytes} bytes → ${brotliBytes} Brotli / ${gzipBytes} gzip`)
