// Exercise the source distribution's real launcher with disposable data.
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { createServer } from 'node:net'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const windows = process.platform === 'win32'
const data = await mkdtemp(path.join(tmpdir(), 'plyback-start-smoke-'))
const env = { ...process.env, PLYBACK_DATA: data, PLYBACK_HOST: '127.0.0.1', PLYBACK_PORT: '8000' }
// Check the launcher's default UI location and explicit port override.
delete env.PLYBACK_FRONTEND
delete env.PLYBACK_PASSWORD
delete env.PLYBACK_SECURE_COOKIES
let child
try {
  if (!env.PLYBACK_STOCKFISH) {
    env.PLYBACK_STOCKFISH = (await readFile(path.join(root, 'data/engine/stockfish.path'), 'utf8')).trim()
  }
  const listener = createServer()
  await new Promise((resolve, reject) => {
    listener.once('error', reject)
    listener.listen(0, '127.0.0.1', resolve)
  })
  const port = String(listener.address().port)
  await new Promise((resolve, reject) => listener.close(error => error ? reject(error) : resolve()))
  assert.notEqual(port, '8000')
  child = windows
    ? spawn('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(root, 'start.ps1'), '-Port', port, '-NoBrowser'], { cwd: root, env, stdio: 'inherit', windowsHide: true })
    : spawn('bash', [path.join(root, 'start.sh'), '--port', port, '--no-browser'], { cwd: root, env, stdio: 'inherit', detached: true })
  let spawnError
  child.on('error', error => { spawnError = error })
  const base = `http://127.0.0.1:${port}`
  let healthy = false
  for (let attempt = 0; attempt < 60; attempt++) {
    if (spawnError) throw spawnError
    assert.equal(child.exitCode, null, 'Launcher exited before becoming healthy')
    assert.equal(child.signalCode, null, 'Launcher was terminated before becoming healthy')
    try {
      const response = await fetch(`${base}/api/health`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) { healthy = true; break }
    } catch { /* Server is still starting. */ }
    await delay(1000)
  }
  assert.ok(healthy, 'Launcher did not serve /api/health within the startup deadline')
  const response = await fetch(base, { signal: AbortSignal.timeout(5000) })
  assert.equal(response.status, 200)
  assert.match(response.headers.get('content-type') ?? '', /text\/html/)
  const html = await response.text()
  assert.match(html, /<title>Plyback<\/title>/)
  assert.match(html, /<div id="root">/)
  console.log(`Setup/start smoke passed on ${process.platform}: health and built UI served on ${port}`)
} finally {
  try {
    if (child?.pid) {
      if (windows) {
        const stopped = spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'inherit', windowsHide: true })
        assert.ok(stopped.status === 0 || child.exitCode !== null, 'Could not stop the launcher process tree')
      } else {
        try { process.kill(-child.pid, 'SIGTERM') } catch (error) { if (error.code !== 'ESRCH') throw error }
      }
      if (child.exitCode === null && child.signalCode === null) {
        await Promise.race([
          new Promise(resolve => child.once('exit', resolve)),
          delay(10_000, undefined, { ref: false }).then(() => { throw new Error('Launcher did not stop') }),
        ])
      }
    }
  } finally { await rm(data, { recursive: true, force: true }) }
}
