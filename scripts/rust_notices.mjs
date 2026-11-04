// Regenerate after Cargo.lock changes. Uses only already-resolved package sources.
import { execFileSync } from 'node:child_process'
import { readFile, readdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--manifest-path', path.join(root, 'rust/Cargo.toml'), '--format-version', '1'], { maxBuffer: 16 * 1024 * 1024, encoding: 'utf8' }))
const sections = ['Plyback Rust dependency notices\nGenerated from rust/Cargo.lock. Includes build, test and target-specific dependencies.\nEach component retains its own copyright and license terms.\n']
const missing = []
const upstream = { 'pgn-reader@0.29.0': 'shakmaty-COPYING', 'shakmaty@0.30.0': 'shakmaty-COPYING', 'rsqlite-vfs@0.2.0': 'rsqlite-vfs-LICENSE', 'valuable@0.1.1': 'valuable-LICENSE' }
for (const pkg of metadata.packages.filter(p => p.name !== 'plyback').sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`))) {
  const directory = path.dirname(pkg.manifest_path)
  sections.push(`\n${'='.repeat(80)}\n${pkg.name} ${pkg.version}\nLicense: ${pkg.license ?? 'See package license file'}\nSource: ${pkg.repository ?? pkg.homepage ?? `https://crates.io/crates/${pkg.name}/${pkg.version}`}\n`)
  const files = new Set(pkg.license_file ? [pkg.license_file] : [])
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isFile() && /^(LICENSE|LICENCE|COPYING|NOTICE|UNLICENSE|COPYRIGHT|AUTHORS)/i.test(entry.name)) files.add(entry.name)
    if (entry.isDirectory() && /^(licenses?|legal)$/i.test(entry.name)) {
      for (const item of await readdir(path.join(directory, entry.name), { withFileTypes: true })) if (item.isFile()) files.add(`${entry.name}/${item.name}`)
    }
  }
  const fallback = upstream[`${pkg.name}@${pkg.version}`]
  if (fallback) sections.push(await readFile(path.join(root, 'docs/licenses/rust-upstream', fallback), 'utf8'))
  if (!files.size && !fallback) missing.push(`${pkg.name}@${pkg.version}`)
  for (const file of [...files].sort()) sections.push(`\n--- ${file} ---\n${await readFile(path.join(directory, file), 'utf8')}`)
}
await writeFile(path.join(root, 'docs/licenses/rust.txt'), sections.join('\n').replace(/\r\n/g, '\n').replace(/[ \t]+$/gm, '').trimEnd() + '\n')
console.log(`Recorded ${metadata.packages.length - 1} package notices.`)
if (missing.length) console.log(`Packages without standalone notice files (review package license metadata): ${missing.join(', ')}`)
