// Assemble a portable release archive from a finished build:
//   node scripts/package_release.mjs --target linux-x64 --version 3.1.0 --stockfish <installer dest> --out <dir>
// The archive runs as unpacked: the server finds frontend/dist and data/engine next to itself.
import { cp, mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

export function parseArgs(argv) {
  const args = {}
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, '')
    if (!['target', 'version', 'stockfish', 'out'].includes(key) || argv[i + 1] === undefined) throw new Error(`Unexpected argument: ${argv[i]}`)
    args[key] = argv[i + 1]
  }
  for (const key of ['target', 'version', 'stockfish', 'out']) if (!args[key]) throw new Error(`--${key} is required`)
  if (!/^(linux|macos|windows)-(x64|arm64)$/.test(args.target)) throw new Error(`Unknown target: ${args.target}`)
  if (!/^\d+\.\d+\.\d+$/.test(args.version)) throw new Error(`Version must look like 1.2.3: ${args.version}`)
  return args
}

export function readme(version, target) {
  const windows = target.startsWith('windows')
  const program = windows ? 'plyback.exe' : './plyback'
  const start = windows ? 'Double-click plyback.exe' : 'Run ./plyback'
  // Release builds are not notarised; macOS blocks quarantined downloads until this is cleared.
  const macos = target.startsWith('macos')
    ? '\nOn macOS, allow the downloaded programs once before the first start:\n  xattr -dr com.apple.quarantine .\n'
    : ''
  return `Plyback ${version}

${start} and open http://localhost:8000.
${macos}
Your games and settings are kept in data/plyback.db. To upgrade, stop Plyback, unpack the
new release and copy data/plyback.db from the old folder into the new data/ folder.

The server listens on this computer only. To reach it from other devices, set a password
first (${program} set-password) and start it with --host 0.0.0.0. See
https://github.com/klemen-r/plyback/blob/main/docs/self-hosting.md

Stockfish (GPL-3.0) is included in data/engine; its licence, authors and the exact source
release are in licenses/stockfish. Plyback is GPL-3.0-or-later; see LICENSE.
`
}

async function main() {
  const { target, version, stockfish, out } = parseArgs(process.argv.slice(2))
  const windows = target.startsWith('windows')
  const exe = windows ? '.exe' : ''
  const name = `plyback-${version}-${target}`
  const dir = path.resolve(out, name)
  await rm(dir, { recursive: true, force: true })
  await mkdir(path.join(dir, 'data', 'engine'), { recursive: true })

  const binary = path.join(process.env.CARGO_TARGET_DIR ?? path.join(root, 'rust', 'target'), 'release', `plyback${exe}`)
  await cp(binary, path.join(dir, `plyback${exe}`))
  const dist = path.join(root, 'frontend', 'dist')
  await stat(path.join(dist, 'index.html'))
  await cp(dist, path.join(dir, 'frontend', 'dist'), { recursive: true })

  // The installer leaves the selected engine at its top level and keeps the release's
  // notices and provenance (manifest.json) beside each installed version.
  await cp(path.join(stockfish, `stockfish${exe}`), path.join(dir, 'data', 'engine', `stockfish${exe}`))
  const versions = await readdir(path.join(stockfish, 'versions'))
  if (versions.length !== 1) throw new Error(`Expected one installed Stockfish version, found ${versions.length}`)
  const installed = path.join(stockfish, 'versions', versions[0])
  const licenses = path.join(dir, 'licenses')
  await mkdir(path.join(licenses, 'stockfish'), { recursive: true })
  for (const file of await readdir(installed)) {
    if (!file.toLowerCase().startsWith('stockfish')) await cp(path.join(installed, file), path.join(licenses, 'stockfish', file))
  }
  JSON.parse(await readFile(path.join(licenses, 'stockfish', 'manifest.json'), 'utf8'))

  await cp(path.join(root, 'LICENSE'), path.join(dir, 'LICENSE'))
  await cp(path.join(root, 'THIRD_PARTY_NOTICES.md'), path.join(licenses, 'THIRD_PARTY_NOTICES.md'))
  await cp(path.join(root, 'docs', 'licenses'), licenses, { recursive: true })
  const text = readme(version, target)
  await writeFile(path.join(dir, windows ? 'README.txt' : 'README'), windows ? text.replace(/\n/g, '\r\n') : text)

  // bsdtar (Windows, macOS) writes zip from the extension; GNU tar on Linux writes tar.gz.
  // Windows: the system bsdtar, not a GNU tar from Git for Windows that cannot write zip.
  const file = `${name}${windows ? '.zip' : '.tar.gz'}`
  const archive = path.resolve(out, file)
  await rm(archive, { force: true })
  const tar = process.platform === 'win32' ? path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe') : 'tar'
  const args = windows ? ['-a', '-c', '-f', file, name] : ['-c', '-z', '-f', file, name]
  const result = spawnSync(tar, args, { cwd: path.resolve(out), stdio: 'inherit' })
  if (result.status !== 0) throw new Error(`tar exited with ${result.status}`)
  console.log(archive)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exit(1) })
}
