// Deterministic Chess.com test double; never contacts a live account.
import http from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const fixtures = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../rust/tests/fixtures/chesscom')
const portIndex = process.argv.indexOf('--port')
const port = portIndex < 0 ? 8799 : Number(process.argv[portIndex + 1])
const lichessPgn = `[Event "Fixture"]
[Site "https://lichess.org/fixture1"]
[White "lichess_fixture"]
[Black "FixtureFriend"]
[Result "0-1"]
[TimeControl "300+0"]

1. f3 e5 2. g4 Qh4# 0-1`
const lichessGame = {
  id: 'fixture1', variant: 'standard', speed: 'blitz', rated: true,
  createdAt: 1735732800000, lastMoveAt: 1735732860000, status: 'mate', winner: 'black',
  players: { white: { user: { name: 'lichess_fixture' }, rating: 1400 }, black: { user: { name: 'FixtureFriend' }, rating: 1450 } },
  clock: { initial: 300, increment: 0 }, pgn: lichessPgn,
}
const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost')
  if (url.pathname === '/api/user/lichess_fixture') {
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify({ id: 'lichess_fixture', username: 'lichess_fixture', createdAt: 1704067200000, perfs: { blitz: { rating: 1400 } } }))
    return
  }
  if (url.pathname === '/api/games/user/lichess_fixture') {
    res.setHeader('Content-Type', 'application/x-ndjson')
    // Write separate chunks to exercise the streaming client without any live requests.
    if (Number(url.searchParams.get('since') ?? 0) <= lichessGame.createdAt) {
      const line = JSON.stringify(lichessGame)
      res.write(line.slice(0, 48))
      res.write(line.slice(48) + '\n')
    }
    res.end()
    return
  }
  if (url.pathname === '/api/games/export/_ids' && req.method === 'POST') {
    let ids = ''
    for await (const chunk of req) ids += chunk
    res.setHeader('Content-Type', 'application/x-ndjson')
    res.end(ids.split(',').includes(lichessGame.id) ? `${JSON.stringify(lichessGame)}\n` : '')
    return
  }
  const match = /^\/pub\/player\/([A-Za-z0-9_-]+)(?:\/(.*))?$/.exec(new URL(req.url, 'http://localhost').pathname)
  res.setHeader('Content-Type', 'application/json')
  if (!match || match[1] !== 'fixture_player') { res.writeHead(404).end('{"message":"not found"}'); return }
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
