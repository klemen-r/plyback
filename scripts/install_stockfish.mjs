// Build/setup helper. The production server does not require Node or Python.
// Only official release assets are fetched; archive paths are never extracted to
// disk. Selected regular file contents are streamed into fixed, bounded targets.
import { createHash, randomUUID } from 'node:crypto'
import { spawn } from 'node:child_process'
import { createWriteStream } from 'node:fs'
import { chmod, copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
// The engine is pinned so every installation analyses with the same Stockfish and an
// archive is accepted only with the SHA-256 recorded here. To move to a new release:
//   gh api repos/official-stockfish/Stockfish/releases/tags/<tag> --jq '.assets[] | "\(.name) \(.digest)"'
// copy the desktop assets below, and bump the analysis version if results change.
export const PINNED = {
  tag: 'sf_19',
  sha256: {
    'stockfish-linux-arm64-universal.tar.gz': 'fe26cfd1d9db4c8af3d21e24d9ff34cacb31c1f940085a7583da11796f2bac01',
    'stockfish-linux-x86-64-universal.tar.gz': '9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f',
    'stockfish-macos-universal.tar.gz': 'a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77',
    'stockfish-windows-arm64-universal.zip': '8372ad3f0d7276deb2c70f801f541ec7db463219fc6d9c7592864e542aa4f401',
    'stockfish-windows-x86-64-universal.zip': '3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7',
  },
}
export const metadataURL = `https://api.github.com/repos/official-stockfish/Stockfish/releases/tags/${PINNED.tag}`
const MAX_ARCHIVE = 512 * 1024 * 1024
const MAX_BINARY = 256 * 1024 * 1024
const MAX_NOTICE = 2 * 1024 * 1024
const executable = process.platform === 'win32' ? 'stockfish.exe' : 'stockfish'
// Windows' own bsdtar reads .zip archives; Git for Windows puts GNU tar, which cannot, earlier on PATH.
export const tar = process.platform === 'win32' ? path.join(process.env.SystemRoot ?? 'C:\\Windows','System32', 'tar.exe') : 'tar'

function candidates() {
  const arm = process.arch === 'arm64'
  if (process.platform === 'darwin') return ['macos-universal', arm ? 'macos-m1-apple-silicon' : 'macos-x86-64-avx2', 'macos-x86-64']
  const os = process.platform === 'win32' ? 'windows' : arm ? 'linux' : 'ubuntu'
  if (arm) return [`${os}-arm64-universal`, 'linux-arm64-universal', `${os}-armv8`]
  return [`${os}-x86-64-universal`, 'linux-x86-64-universal', `${os}-x86-64-avx2`, `${os}-x86-64-sse41-popcnt`, `${os}-x86-64`]
}

export async function response(url, asset = false, { fetchImpl = globalThis.fetch, githubToken = process.env.GITHUB_TOKEN } = {}) {
  const original = new URL(url)
  const safe = target => {
    try {
      const decodedPath = decodeURIComponent(target.pathname)
      return target.protocol === 'https:' && !target.username && !target.password && !target.port && !target.hash
        && !decodedPath.split('/').includes('..') && !/[\\\u0000-\u001f\u007f]/.test(decodedPath)
    } catch { return false }
  }
  const officialAsset = target => target.hostname === 'github.com' && target.pathname.startsWith('/official-stockfish/Stockfish/releases/download/')
  if (!safe(original)) throw new Error('Invalid release URL')
  if (asset && !officialAsset(original)) throw new Error('Asset is outside official Stockfish releases')
  if (!asset && url !== metadataURL && !(original.hostname === 'raw.githubusercontent.com' && original.pathname.startsWith('/official-stockfish/Stockfish/'))) throw new Error('Metadata is outside official Stockfish sources')
  for (let redirects = 0; redirects <= 5; redirects++) {
    const headers = { 'User-Agent': 'plyback-installer', Accept: 'application/json, application/octet-stream' }
    // Rebuild headers for every hop. Credentials belong only to the exact
    // release API, never to archive CDNs, license sources or redirect targets.
    if (url === metadataURL && githubToken) {
      if (!/^[\x21-\x7e]+$/.test(githubToken)) throw new Error('GITHUB_TOKEN must contain only visible ASCII characters')
      headers.Authorization = `Bearer ${githubToken}`
    }
    const reply = await fetchImpl(url, { redirect: 'manual', signal: AbortSignal.timeout(120_000), headers })
    if ([301, 302, 303, 307, 308].includes(reply.status)) {
      await reply.body?.cancel()
      const location = reply.headers.get('location')
      if (!location) throw new Error('Release redirect has no location')
      const next = new URL(location, url)
      // A github.com redirect must retain the official repository boundary.
      // Metadata/license redirects cannot turn into arbitrary release downloads.
      const allowed = asset
        ? officialAsset(next) || ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'].includes(next.hostname)
        : next.origin === original.origin && next.pathname === original.pathname
      if (!safe(next) || !allowed) throw new Error('Unexpected release redirect')
      url = next.href
      continue
    }
    if (!reply.ok) { await reply.body?.cancel(); throw new Error(`Official release request returned ${reply.status}`) }
    return reply
  }
  throw new Error('Too many release redirects')
}

export async function boundedBytes(reply, limit) {
  const chunks = []; let total = 0
  for await (const chunk of reply.body) {
    total += chunk.length
    if (total > limit) throw new Error('Release metadata exceeds the size limit')
    chunks.push(chunk)
  }
  return Buffer.concat(chunks)
}

export async function download(url, destination, expected, { fetchImpl, limit = MAX_ARCHIVE } = {}) {
  if (expected && !/^sha256:[a-f0-9]{64}$/i.test(expected)) throw new Error('Unsupported release digest')
  const reply = await response(url, true, { fetchImpl })
  const stream = createWriteStream(destination, { flags: 'wx' })
  const digest = createHash('sha256'); let total = 0
  const checked = new Transform({ transform(chunk, _encoding, next) {
    total += chunk.length
    if (total > limit) { next(new Error('Stockfish archive exceeds the size limit')); return }
    digest.update(chunk)
    next(null, chunk)
  } })
  // pipeline propagates filesystem errors and size failures back to the HTTP
  // body, cancelling the download even when a file write is backpressured.
  await pipeline(reply.body, checked, stream)
  const actual = digest.digest('hex')
  if (expected && actual !== expected.slice(7).toLowerCase()) throw new Error('Stockfish archive SHA-256 mismatch')
  return actual
}

export async function command(command, args, { input, limit = MAX_NOTICE, timeout = 30_000, spawnImpl = spawn } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawnImpl(command, args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
    const chunks = []; let size = 0; let error = ''
    const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`${command} timed out`)) }, timeout)
    child.on('error', err => { clearTimeout(timer); reject(err) })
    child.stdout.on('data', chunk => { size += chunk.length; if (size > limit) { child.kill('SIGKILL'); reject(new Error('Archive member exceeds the size limit')) } else chunks.push(chunk) })
    child.stderr.on('data', chunk => { if (error.length < 2000) error += chunk.toString().slice(0, 2000 - error.length) })
    child.on('close', code => { clearTimeout(timer); if (code !== 0) reject(new Error(`${command} failed (${code}): ${error.slice(0, 500)}`)); else resolve(Buffer.concat(chunks)) })
    child.stdin.on('error', () => {})
    child.stdin.end(input ?? '')
  })
}

async function verify(binary, runCommand = command) {
  const output = (await runCommand(binary, [], { input: 'uci\nquit\n' })).toString()
  const lines = output.split(/\r?\n/)
  const name = lines.find(line => line.startsWith('id name '))?.slice(8).trim()
  if (!name || !lines.includes('uciok')) throw new Error('Engine did not complete the UCI handshake')
  return name
}

export async function extract(archive, output, { runCommand = command, binaryLimit = MAX_BINARY, noticeLimit = MAX_NOTICE } = {}) {
  await mkdir(output)
  // tar -O emits file contents on stdout; it cannot write a traversal path,
  // replace a file, follow a destination symlink, or create a device node.
  const entries = (await runCommand(tar, ['-tf', archive], { limit: 4 * noticeLimit })).toString().split(/\r?\n/).filter(Boolean)
  const binaries = entries.filter(name => {
    const base = path.posix.basename(name).toLowerCase()
    return base.startsWith('stockfish') && !name.endsWith('/') && (process.platform === 'win32' ? base.endsWith('.exe') : !base.includes('.'))
  })
  if (binaries.length !== 1) throw new Error('Expected one Stockfish binary in the official archive')
  const binary = path.join(output, executable)
  await writeFile(binary, await runCommand(tar, ['-xOf', archive, '--', binaries[0]], { limit: binaryLimit, timeout: 120_000 }), { flag: 'wx' })
  await chmod(binary, 0o755)
  for (const [name, target] of [['copying.txt', 'Copying.txt'], ['authors', 'AUTHORS'], ['readme.md', 'README.md']]) {
    const member = entries.find(entry => path.posix.basename(entry).toLowerCase() === name)
    if (member) await writeFile(path.join(output, target), await runCommand(tar, ['-xOf', archive, '--', member], { limit: noticeLimit }), { flag: 'wx' })
  }
  return binary
}

export async function install(destination, force = false, { fetchImpl, runCommand = command, limits = {}, pin = PINNED } = {}) {
  const { archive: archiveLimit = MAX_ARCHIVE, binary: binaryLimit = MAX_BINARY, notice: noticeLimit = MAX_NOTICE } = limits
  destination = path.resolve(destination)
  if (!force) {
    try {
      const existing = (await readFile(path.join(destination, 'stockfish.path'), 'utf8')).trim()
      console.log(`Stockfish ready: ${await verify(existing, runCommand)}`)
      return existing
    } catch { /* Validate a staged replacement before publishing it. */ }
  }
  const release = JSON.parse((await boundedBytes(await response(metadataURL, false, { fetchImpl }), noticeLimit)).toString())
  if (release.tag_name !== pin.tag) throw new Error(`Expected Stockfish release ${pin.tag}, got ${String(release.tag_name)}`)
  const assets = []; const seen = new Set()
  for (const candidate of candidates()) for (const asset of release.assets ?? []) {
    if (typeof asset.name === 'string' && asset.name.includes(candidate) && /\.(zip|tar|tar\.gz|tgz)$/.test(asset.name) && !seen.has(asset.name)) { assets.push(asset); seen.add(asset.name) }
  }
  const failures = []
  for (const asset of assets) {
    await mkdir(destination, { recursive: true })
    const stage = await mkdtemp(path.join(destination, '.stockfish-'))
    try {
      console.log(`Installing official ${release.tag_name}: ${asset.name}`)
      const archive = path.join(stage, 'download')
      const pinned = Object.hasOwn(pin.sha256, asset.name) ? `sha256:${pin.sha256[asset.name]}` : null
      if (!pinned) throw new Error('Asset has no pinned SHA-256')
      if (asset.digest && asset.digest.toLowerCase() !== pinned) throw new Error('Release digest differs from the pinned SHA-256')
      const digest = await download(asset.browser_download_url, archive, pinned, { fetchImpl, limit: archiveLimit })
      const output = path.join(stage, 'engine')
      const binary = await extract(archive, output, { runCommand, binaryLimit, noticeLimit })
      const name = await verify(binary, runCommand)
      try { await readFile(path.join(output, 'Copying.txt')) } catch {
        const license = await boundedBytes(await response(`https://raw.githubusercontent.com/official-stockfish/Stockfish/${encodeURIComponent(release.tag_name)}/Copying.txt`, false, { fetchImpl }), noticeLimit)
        await writeFile(path.join(output, 'Copying.txt'), license)
      }
      const manifest = { engine: name, release: release.tag_name, asset: asset.name, asset_url: asset.browser_download_url, archive_sha256: digest, pinned_sha256_verified: true, release_digest_verified: Boolean(asset.digest), source_url: `https://github.com/official-stockfish/Stockfish/archive/refs/tags/${encodeURIComponent(release.tag_name)}.tar.gz`, license: 'GPL-3.0' }
      await writeFile(path.join(output, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n')
      await mkdir(path.join(destination, 'versions'), { recursive: true })
      const installed = path.join(destination, 'versions', randomUUID())
      const selected = path.join(installed, executable)
      await copyFile(binary, path.join(stage, executable))
      await writeFile(path.join(stage, 'stockfish.path'), selected)
      await writeFile(path.join(stage, 'STOCKFISH-NOTICES.txt'), 'Stockfish is GPL-3.0. Each installed version retains Copying.txt, available author notices and manifest.json with the exact release source URL under versions/. stockfish.path selects the active version.\n')
      await rename(output, installed)
      await rename(path.join(stage, executable), path.join(destination, executable))
      await rename(path.join(stage, 'STOCKFISH-NOTICES.txt'), path.join(destination, 'STOCKFISH-NOTICES.txt'))
      // Publish the authoritative selector last: every referenced version is
      // complete, verified and retains its own license/provenance files.
      await rename(path.join(stage, 'stockfish.path'), path.join(destination, 'stockfish.path'))
      console.log(`Stockfish ready: ${name}`)
      return selected
    } catch (error) { failures.push(`${asset.name}: ${error.message}`) }
    finally { await rm(stage, { recursive: true, force: true }) }
  }
  throw new Error(`Could not install Stockfish automatically.\n${failures.join('\n')}\nInstall from https://stockfishchess.org/download/ and set PLYBACK_STOCKFISH. A tar utility is required for automatic installation.`)
}

export function engineDestination(args, dataDirectory) {
  const index = args.indexOf('--dest')
  const destination = index < 0 ? path.join(dataDirectory || path.join(root, 'data'), 'engine') : args[index + 1]
  if (!destination) throw new Error('--dest needs a directory')
  return destination
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await install(engineDestination(process.argv.slice(2), process.env.PLYBACK_DATA), process.argv.includes('--force'))
}
