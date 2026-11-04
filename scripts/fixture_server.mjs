// Deterministic Chess.com test double; never contacts a live account.
import http from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const fixtures = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../backend/tests/fixtures/chesscom')
const portIndex = process.argv.indexOf('--port')
const port = portIndex < 0 ? 8799 : Number(process.argv[portIndex + 1])
const server = http.createServer(async (req, res) => {
  const match = /^\/pub\/player\/([A-Za-z0-9_-]+)(?:\/(.*))?$/.exec(new URL(req.url, 'http://localhost').pathname)
  res.setHeader('Content-Type', 'application/json')
  if (!match || match[1] !== 'klemen_r') { res.writeHead(404).end('{"message":"not found"}'); return }
  const suffix = match[2] ?? ''
  let file = suffix === '' ? 'profile.json' : suffix === 'stats' ? 'stats.json' : suffix === 'games/archives' ? 'archives.json' : null
  const month = /^games\/(\d{4})\/(\d{2})$/.exec(suffix)
  if (month) {
    file = `${month[1]}-${month[2]}.json`
    const etag = `"${month[1]}${month[2]}"`
    if (req.headers['if-none-match'] === etag) { res.writeHead(304).end(); return }
    res.setHeader('ETag', etag)
  }
  if (!file) { res.writeHead(404).end('{"message":"not found"}'); return }
  try { res.end(await readFile(path.join(fixtures, file))) }
  catch { res.end('{"games":[]}') }
})
server.listen(port, '127.0.0.1')
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => server.close(() => process.exit(0)))
