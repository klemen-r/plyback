import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { EventEmitter } from 'node:events'
import { access, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { PassThrough } from 'node:stream'
import test from 'node:test'
import { boundedBytes, command, download, engineDestination, extract, install, metadataURL, PINNED, response, tar } from './install_stockfish.mjs'

const assetURL = 'https://github.com/official-stockfish/Stockfish/releases/download/sf_test/stockfish.tar'
const licenseURL = 'https://raw.githubusercontent.com/official-stockfish/Stockfish/sf_test/Copying.txt'
const executable = process.platform === 'win32' ? 'stockfish.exe' : 'stockfish'
const hash = bytes => createHash('sha256').update(bytes).digest('hex')

test('the installer follows PLYBACK_DATA unless a destination is explicit', () => {
  assert.equal(engineDestination([], 'custom directory'), path.join('custom directory', 'engine'))
  assert.equal(engineDestination(['--dest', 'explicit directory'], 'custom directory'), 'explicit directory')
  assert.throws(() => engineDestination(['--dest'], 'custom directory'), /--dest needs a directory/)
})

async function temporary(t) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'plyback-installer-test-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return directory
}

function streamed(chunks, cancelled = () => {}) {
  let index = 0
  return new ReadableStream({
    pull(controller) {
      if (index === chunks.length) controller.close()
      else controller.enqueue(Buffer.from(chunks[index++]))
    },
    cancel: cancelled,
  })
}

test('untrusted initial URLs are rejected before any network request', async () => {
  const forbidden = async () => assert.fail('Unexpected network request')
  for (const [url, asset] of [
    ['http://github.com/official-stockfish/Stockfish/releases/download/x/y.tar', true],
    ['https://user:password@github.com/official-stockfish/Stockfish/releases/download/x/y.tar', true],
    ['https://github.com:8443/official-stockfish/Stockfish/releases/download/x/y.tar', true],
    [assetURL + '#fragment', true],
    ['https://github.com.evil.invalid/official-stockfish/Stockfish/releases/download/x/y.tar', true],
    ['https://github.com/another-owner/Stockfish/releases/download/x/y.tar', true],
    ['https://github.com/official-stockfish/Stockfish/releases/download/..%2F..%2F..%2Funtrusted/asset.tar', true],
    ['https://github.com/official-stockfish/Stockfish/releases/download/x%5C..%5Cy/asset.tar', true],
    ['https://release-assets.githubusercontent.com/asset.tar', true],
    ['https://127.0.0.1/private', true],
    [metadataURL + '/extra', false],
    ['https://raw.githubusercontent.com/another-owner/Stockfish/main/Copying.txt', false],
  ]) await assert.rejects(response(url, asset, { fetchImpl: forbidden }), /Invalid|outside/)
})

test('each redirect stays inside the official release boundary and cancels its body', async () => {
  for (const location of [
    'http://release-assets.githubusercontent.com/asset',
    'https://release-assets.githubusercontent.com.evil.invalid/asset',
    'https://user:password@release-assets.githubusercontent.com/asset',
    'https://release-assets.githubusercontent.com:8443/asset',
    'https://github.com/untrusted/repository/releases/download/x/y.tar',
    'https://github.com/official-stockfish/Stockfish/issues/1',
    'https://github.com/official-stockfish/Stockfish/releases/download/..%2F..%2F..%2Funtrusted/asset.tar',
    'https://127.0.0.1/private',
  ]) {
    let calls = 0; let cancelled = false
    await assert.rejects(response(assetURL, true, { fetchImpl: async () => {
      calls++
      return new Response(streamed(['redirect body'], () => { cancelled = true }), { status: 302, headers: { location } })
    } }), /Unexpected release redirect/)
    assert.equal(calls, 1)
    assert.equal(cancelled, true)
  }
})

test('official CDN redirects are followed manually, including signed queries', async () => {
  const urls = [assetURL, 'https://release-assets.githubusercontent.com/signed?token=fixture', 'https://objects.githubusercontent.com/final']
  const seen = []
  const reply = await response(assetURL, true, { fetchImpl: async (url, options) => {
    seen.push(url)
    assert.equal(options.redirect, 'manual')
    assert.ok(options.signal instanceof AbortSignal)
    const index = urls.indexOf(url)
    assert.notEqual(index, -1)
    return index === 2 ? new Response('archive') : new Response('', { status: 307, headers: { location: urls[index + 1] } })
  } })
  assert.equal(await reply.text(), 'archive')
  assert.deepEqual(seen, urls)
})

test('a valid CDN hop does not authorize a later untrusted redirect', async () => {
  const cdn = 'https://release-assets.githubusercontent.com/fixture'
  const seen = []
  await assert.rejects(response(assetURL, true, { fetchImpl: async url => {
    seen.push(url)
    return new Response('', { status: 302, headers: { location: url === assetURL ? cdn : 'http://127.0.0.1/private' } })
  } }), /Unexpected release redirect/)
  assert.deepEqual(seen, [assetURL, cdn])
})

test('metadata and license requests cannot redirect to release assets or unrelated paths', async () => {
  for (const source of [metadataURL, licenseURL]) {
    await assert.rejects(response(source, false, { fetchImpl: async () => new Response('', { status: 302, headers: { location: assetURL } }) }), /Unexpected release redirect/)
    const reply = await response(source, false, { fetchImpl: async () => new Response('official source') })
    assert.equal(await reply.text(), 'official source')
  }
})

test('GITHUB_TOKEN authenticates only the exact release API and never archive or license requests', async t => {
  const previous = process.env.GITHUB_TOKEN
  process.env.GITHUB_TOKEN = 'fixture-github-token'
  t.after(() => {
    if (previous === undefined) delete process.env.GITHUB_TOKEN
    else process.env.GITHUB_TOKEN = previous
  })
  const seen = []
  const cdn = 'https://release-assets.githubusercontent.com/signed?token=fixture'
  const fetchImpl = async (url, options) => {
    seen.push([url, options.headers.Authorization])
    return url === assetURL
      ? new Response('', { status: 302, headers: { location: cdn } })
      : new Response('source')
  }
  await response(metadataURL, false, { fetchImpl })
  await response(licenseURL, false, { fetchImpl })
  await response(assetURL, true, { fetchImpl })
  await response(metadataURL, false, { fetchImpl, githubToken: '' })
  assert.deepEqual(seen, [
    [metadataURL, 'Bearer fixture-github-token'],
    [licenseURL, undefined],
    [assetURL, undefined],
    [cdn, undefined],
    [metadataURL, undefined],
  ])
})

test('metadata redirects cannot forward credentials to another endpoint', async () => {
  const token = 'fixture-github-token'
  for (const target of [licenseURL, assetURL, metadataURL + '/other', 'https://api.github.com.evil.invalid/']) {
    let calls = 0
    await assert.rejects(response(metadataURL, false, { githubToken: token, fetchImpl: async (_url, options) => {
      calls++
      assert.equal(options.headers.Authorization, `Bearer ${token}`)
      return new Response('', { status: 302, headers: { location: target } })
    } }), /Unexpected release redirect/)
    assert.equal(calls, 1)
  }
  const target = metadataURL + '?redirected=1'
  const seen = []
  await response(metadataURL, false, { githubToken: token, fetchImpl: async (url, options) => {
    seen.push(options.headers.Authorization)
    return url === metadataURL ? new Response('', { status: 302, headers: { location: target } }) : new Response('release')
  } })
  assert.deepEqual(seen, [`Bearer ${token}`, undefined])
})

test('invalid tokens and API errors do not include credentials in diagnostics', async () => {
  await assert.rejects(response(metadataURL, false, { githubToken: 'fixture-secret\ninvalid', fetchImpl: async () => assert.fail('Unexpected fetch') }), error => {
    assert.match(error.message, /GITHUB_TOKEN/)
    assert.ok(!error.message.includes('fixture-secret'))
    return true
  })
  await assert.rejects(response(metadataURL, false, { githubToken: 'fixture-secret', fetchImpl: async () => new Response('fixture-secret', { status: 403 }) }), error => {
    assert.equal(error.message, 'Official release request returned 403')
    return true
  })
})

test('redirect loops, missing locations and HTTP failures terminate without extra requests', async () => {
  let calls = 0
  await assert.rejects(response(assetURL, true, { fetchImpl: async () => {
    calls++
    return new Response('', { status: 302, headers: { location: assetURL } })
  } }), /Too many release redirects/)
  assert.equal(calls, 6)
  await assert.rejects(response(assetURL, true, { fetchImpl: async () => new Response('', { status: 302 }) }), /no location/)
  let cancelled = false
  await assert.rejects(response(assetURL, true, { fetchImpl: async () => new Response(streamed(['error'], () => { cancelled = true }), { status: 503 }) }), /503/)
  assert.equal(cancelled, true)
})

test('metadata bounds use streamed bytes even without a Content-Length header', async () => {
  assert.equal((await boundedBytes(new Response(streamed(['ab', 'cd'])), 4)).toString(), 'abcd')
  let cancelled = false
  await assert.rejects(boundedBytes(new Response(streamed(['abcde', 'unused', 'unused'], () => { cancelled = true })), 4), /size limit/)
  assert.equal(cancelled, true)
})

test('downloads verify the digest and reject unsupported digests before fetching', async t => {
  const directory = await temporary(t)
  const bytes = Buffer.from('tiny archive fixture')
  const fetchImpl = async () => new Response(bytes)
  const target = path.join(directory, 'archive')
  assert.equal(await download(assetURL, target, `SHA256:${hash(bytes).toUpperCase()}`, { fetchImpl }), hash(bytes))
  assert.deepEqual(await readFile(target), bytes)
  await assert.rejects(download(assetURL, path.join(directory, 'bad'), `sha256:${'0'.repeat(64)}`, { fetchImpl }), /SHA-256 mismatch/)
  await assert.rejects(download(assetURL, path.join(directory, 'unsupported'), 'md5:fixture', { fetchImpl: async () => assert.fail('Unexpected fetch') }), /Unsupported release digest/)
})

test('downloads stop and cancel oversized bodies and never overwrite an existing file', async t => {
  const directory = await temporary(t)
  let cancelled = false
  const target = path.join(directory, 'bounded')
  await assert.rejects(download(assetURL, target, null, {
    limit: 4,
    fetchImpl: async () => new Response(streamed(['abcde', 'unused', 'unused'], () => { cancelled = true })),
  }), /size limit/)
  assert.equal(cancelled, true)
  assert.ok((await readFile(target)).length <= 4)
  await writeFile(path.join(directory, 'existing'), 'preserve me')
  await assert.rejects(download(assetURL, path.join(directory, 'existing'), null, { fetchImpl: async () => new Response('replacement') }), /EEXIST/)
  assert.equal(await readFile(path.join(directory, 'existing'), 'utf8'), 'preserve me')
})

function fakeProcess(start = () => {}) {
  const child = new EventEmitter()
  child.stdin = new PassThrough(); child.stdout = new PassThrough(); child.stderr = new PassThrough()
  const kills = []
  child.kill = signal => { kills.push(signal); queueMicrotask(() => child.emit('close', null)); return true }
  return { child, kills, spawnImpl: (program, args, options) => {
    assert.equal(program, 'mock-tar')
    assert.deepEqual(args, ['fixture'])
    assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe'])
    queueMicrotask(() => start(child))
    return child
  } }
}

test('archive subprocess output bounds kill the child without invoking a real process', async () => {
  const process = fakeProcess(child => child.stdout.write('too large'))
  await assert.rejects(command('mock-tar', ['fixture'], { limit: 4, spawnImpl: process.spawnImpl }), /size limit/)
  assert.deepEqual(process.kills, ['SIGKILL'])
})

test('archive subprocess timeouts kill the child and failures retain bounded diagnostics', async () => {
  const idle = fakeProcess()
  await assert.rejects(command('mock-tar', ['fixture'], { timeout: 5, spawnImpl: idle.spawnImpl }), /timed out/)
  assert.deepEqual(idle.kills, ['SIGKILL'])
  const failed = fakeProcess(child => { child.stderr.write('x'.repeat(5000)); child.emit('close', 7) })
  await assert.rejects(command('mock-tar', ['fixture'], { spawnImpl: failed.spawnImpl }), error => {
    assert.match(error.message, /failed \(7\)/)
    assert.ok(error.message.length < 600)
    return true
  })
})

test('extraction streams member contents into fixed paths and supplies separate output budgets', async t => {
  const directory = await temporary(t)
  const output = path.join(directory, 'engine')
  const binaryMember = `../escaped/${executable}`
  const noticeMember = '../escaped/Copying.txt'
  const calls = []
  const binary = await extract('fixture-archive', output, { binaryLimit: 8, noticeLimit: 64, runCommand: async (program, args, options) => {
    calls.push({ program, args, options })
    if (args[0] === '-tf') return Buffer.from(`${binaryMember}\n${noticeMember}\n`)
    assert.deepEqual(args.slice(0, 3), ['-xOf', 'fixture-archive', '--'])
    return Buffer.from(args[3] === binaryMember ? 'engine' : 'license')
  } })
  assert.equal(binary, path.join(output, executable))
  assert.deepEqual((await readdir(output)).sort(), ['Copying.txt', executable].sort())
  assert.equal(await readFile(binary, 'utf8'), 'engine')
  assert.equal(await readFile(path.join(output, 'Copying.txt'), 'utf8'), 'license')
  assert.deepEqual(calls.map(call => call.options.limit), [256, 8, 64])
  assert.equal(calls[1].options.timeout, 120_000)
  await assert.rejects(access(path.join(directory, 'escaped')), /ENOENT/)
})

test('an ambiguous or missing binary is rejected before any member extraction', async t => {
  const directory = await temporary(t)
  for (const [index, entries] of ['', `a/${executable}\nb/${executable}\n`].entries()) {
    let calls = 0
    await assert.rejects(extract('fixture-archive', path.join(directory, `engine-${index}`), { runCommand: async (_program, args) => {
      calls++
      assert.equal(args[0], '-tf')
      return Buffer.from(entries)
    } }), /Expected one Stockfish binary/)
    assert.equal(calls, 1)
  }
})

function installationFixture({ digest, handshake = true, includeLicense = true, archiveLimit, pinned } = {}) {
  const archive = Buffer.from('mock archive bytes')
  // One portable fixture name matches every supported host; its bytes are never
  // executed or passed to a real tar process.
  const assetName = 'stockfish-macos-universal-windows-x86-64-universal-windows-arm64-universal-ubuntu-x86-64-universal-linux-arm64-universal.tar'
  const release = { tag_name: 'sf_test', assets: [{ name: assetName, browser_download_url: assetURL, digest: digest ?? `sha256:${hash(archive)}` }] }
  const requests = []; const commands = []
  return {
    archive, assetName, requests, commands,
    options: {
      limits: { archive: archiveLimit ?? 128, binary: 128, notice: 1024 },
      pin: { tag: 'sf_test', sha256: pinned ?? { [assetName]: hash(archive) } },
      fetchImpl: async url => {
        requests.push(url)
        if (url === metadataURL) return new Response(JSON.stringify(release))
        if (url === assetURL) return new Response(archive)
        if (url === licenseURL) return new Response('mock GPL license')
        assert.fail(`Unexpected request ${url}`)
      },
      runCommand: async (program, args, options) => {
        commands.push({ program, args, options })
        if (program === tar) {
          if (args[0] === '-tf') return Buffer.from(`release/${executable}\n${includeLicense ? 'release/Copying.txt\n' : ''}`)
          assert.deepEqual(args.slice(0, 1), ['-xOf'])
          assert.equal(args[2], '--')
          return Buffer.from(args[3].endsWith('Copying.txt') ? 'mock GPL license' : 'mock binary')
        }
        assert.equal(path.basename(program), executable)
        assert.deepEqual(args, [])
        assert.equal(options.input, 'uci\nquit\n')
        return Buffer.from(`id name Mock Stockfish\n${handshake ? 'uciok\n' : ''}`)
      },
    },
  }
}

async function previousInstallation(directory) {
  const version = path.join(directory, 'versions', 'previous')
  await mkdir(version, { recursive: true })
  const selected = path.join(version, executable)
  await writeFile(selected, 'previous version')
  await writeFile(path.join(directory, executable), 'previous version')
  await writeFile(path.join(directory, 'stockfish.path'), selected)
  await writeFile(path.join(directory, 'STOCKFISH-NOTICES.txt'), 'previous notices')
  return selected
}

test('publication retains the old version and selects a complete verified version with provenance', async t => {
  const directory = await temporary(t)
  const previous = await previousInstallation(directory)
  const fixture = installationFixture({ includeLicense: false })
  const selected = await install(directory, true, fixture.options)
  assert.notEqual(selected, previous)
  assert.equal(await readFile(path.join(directory, 'stockfish.path'), 'utf8'), selected)
  assert.equal(await readFile(previous, 'utf8'), 'previous version')
  assert.equal(await readFile(selected, 'utf8'), 'mock binary')
  assert.equal(await readFile(path.join(directory, executable), 'utf8'), 'mock binary')
  const installed = path.dirname(selected)
  assert.equal(await readFile(path.join(installed, 'Copying.txt'), 'utf8'), 'mock GPL license')
  const manifest = JSON.parse(await readFile(path.join(installed, 'manifest.json'), 'utf8'))
  assert.equal(manifest.archive_sha256, hash(fixture.archive))
  assert.equal(manifest.release_digest_verified, true)
  assert.equal(manifest.asset_url, assetURL)
  assert.equal(manifest.source_url, 'https://github.com/official-stockfish/Stockfish/archive/refs/tags/sf_test.tar.gz')
  assert.ok(fixture.requests.includes(licenseURL))
  assert.ok(!(await readdir(directory)).some(entry => entry.startsWith('.stockfish-')))
})

test('digest, download-budget and UCI failures preserve the selected engine and clean staging', async t => {
  const directory = await temporary(t)
  for (const [index, settings] of [{ digest: `sha256:${'0'.repeat(64)}` }, { pinned: {} }, { archiveLimit: 4 }, { handshake: false }].entries()) {
    const destination = path.join(directory, String(index))
    const previous = await previousInstallation(destination)
    const fixture = installationFixture(settings)
    await assert.rejects(install(destination, true, fixture.options), /Could not install Stockfish automatically/)
    assert.equal(await readFile(path.join(destination, 'stockfish.path'), 'utf8'), previous)
    assert.equal(await readFile(path.join(destination, executable), 'utf8'), 'previous version')
    assert.equal(await readFile(path.join(destination, 'STOCKFISH-NOTICES.txt'), 'utf8'), 'previous notices')
    assert.deepEqual(await readdir(path.join(destination, 'versions')), ['previous'])
    assert.ok(!(await readdir(destination)).some(entry => entry.startsWith('.stockfish-')))
    if (index < 2) assert.equal(fixture.commands.length, 0)
  }
})

test('a valid existing selection avoids metadata requests and archive work', async t => {
  const directory = await temporary(t)
  const previous = await previousInstallation(directory)
  const fixture = installationFixture()
  assert.equal(await install(directory, false, fixture.options), previous)
  assert.deepEqual(fixture.requests, [])
  assert.equal(fixture.commands.length, 1)
  assert.equal(fixture.commands[0].program, previous)
})

test('the pinned release covers each desktop platform with a SHA-256', () => {
  assert.ok(metadataURL.endsWith(`/releases/tags/${PINNED.tag}`))
  for (const platform of ['linux-x86-64', 'linux-arm64', 'macos', 'windows-x86-64', 'windows-arm64']) {
    assert.ok(Object.keys(PINNED.sha256).some(name => name.includes(platform)), platform)
  }
  for (const digest of Object.values(PINNED.sha256)) assert.match(digest, /^[a-f0-9]{64}$/)
})
