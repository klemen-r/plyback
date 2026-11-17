// Starts only the native server, with an OS-owned temporary data directory.
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { spawn, spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const PREFIX = 'plyback-e2e-'
const OWNERS = 'owners.json'

function alive(pid) {
  try { process.kill(pid, 0); return true } catch (error) { return error.code === 'EPERM' }
}

// Windows test runners force-kill this process tree, so the exit handlers below
// never run there. Remove earlier directories whose processes have all exited;
// directories from older versions without an owner file go after a day.
async function sweepStale() {
  for (const name of await readdir(tmpdir()).catch(() => [])) {
    if (!name.startsWith(PREFIX)) continue
    const dir = path.join(tmpdir(), name)
    try {
      const owners = JSON.parse(await readFile(path.join(dir, OWNERS), 'utf8'))
      if (owners.some(alive)) continue
    } catch {
      const info = await stat(dir).catch(() => null)
      if (!info || Date.now() - info.mtimeMs < 86_400_000) continue
    }
    await rm(dir, { recursive: true, force: true }).catch(() => {})
  }
}

await sweepStale()
const data = await mkdtemp(path.join(tmpdir(), PREFIX))
await writeFile(path.join(data, OWNERS), JSON.stringify([process.pid]))
const binary = process.env.PLYBACK_BINARY ?? path.join(root, 'rust/target/release', process.platform === 'win32' ? 'plyback.exe' : 'plyback')
const env = { ...process.env, PLYBACK_DATA: data, PLYBACK_PORT: process.env.PLYBACK_PORT ?? '8790', PLYBACK_HOST: '127.0.0.1', PLYBACK_FRONTEND: path.join(root, 'frontend/dist'), PLYBACK_CHESSCOM_API: 'http://127.0.0.1:8799/pub', PLYBACK_LICHESS_API: 'http://127.0.0.1:8799' }
// A developer's deployment settings must not leak into this isolated test instance.
delete env.PLYBACK_PASSWORD
delete env.PLYBACK_SECURE_COOKIES
delete env.PLYBACK_NO_BACKGROUND
if (!env.PLYBACK_STOCKFISH) {
  try { env.PLYBACK_STOCKFISH = (await readFile(path.join(root, 'data/engine/stockfish.path'), 'utf8')).trim() } catch { /* PATH lookup */ }
}
const setup = spawnSync(binary, ['settings', JSON.stringify({ analysis_depth: 10, workers: 2, threads_per_worker: 1, max_seconds_per_position: 1, interactive_depth: 12 })], { env, stdio: 'inherit' })
if (setup.status !== 0) { await rm(data, { recursive: true, force: true }); throw new Error('Native test server setup failed; build Rust first') }
const child = spawn(binary, [], { env, stdio: 'inherit' })
if (child.pid) await writeFile(path.join(data, OWNERS), JSON.stringify([process.pid, child.pid]))
let cleaning = false
async function cleanup(code) {
  if (cleaning) return
  cleaning = true
  await rm(data, { recursive: true, force: true })
  process.exit(code ?? 1)
}
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill('SIGINT'))
child.on('error', error => { console.error(error.message); void cleanup(1) })
child.on('exit', code => void cleanup(code))
