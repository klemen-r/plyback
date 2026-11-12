import { useMutation, useQueryClient } from '@tanstack/react-query'
import { FileUp, Upload } from 'lucide-react'
import { useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { PageHeader, Button, Card, ErrorNote, Switch } from '../components/ui'
import { api } from '../lib/api'

/** Files larger than this are imported without showing their text. */
const INLINE_LIMIT = 200_000
const MAX_GAMES = 10_000

export default function ImportPgn() {
  const qc = useQueryClient()
  const [pgn, setPgn] = useState('')
  /** A large file is kept out of the text box: rendering megabytes there freezes the page. */
  const [file, setFile] = useState<{ name: string; size: number; text: string; games: number } | null>(null)
  const [fileError, setFileError] = useState<string | null>(null)
  const [reading, setReading] = useState(false)
  const fileRead = useRef(0)
  const [analyze, setAnalyze] = useState(true)
  const text = file?.text ?? pgn
  const imp = useMutation({ mutationFn: () => api.importPgn(text, analyze), onSuccess: () => {
    for (const key of ['games', 'accounts', 'insights', 'status', 'openings']) qc.invalidateQueries({ queryKey: [key] })
  } })
  const onFile = async (f: File | undefined) => {
    if (!f) return
    const read = ++fileRead.current
    setPgn('')
    setFile(null)
    setFileError(null)
    imp.reset()
    // The server accepts PGN text up to 20,000,000 bytes.
    if (f.size > 20_000_000) {
      setReading(false)
      setFileError('Choose a PGN file of at most 20 MB, or split it into smaller files.')
      return
    }
    setReading(true)
    const content = await f.text().catch(() => null)
    if (read !== fileRead.current) return
    setReading(false)
    if (content === null) {
      setFileError('The selected file could not be read. Try choosing it again.')
      return
    }
    // An estimate from the first tag of each game (files without [Event] tags fall back to [White]).
    // Tags inside comments or a broken last game also count, so it only informs: the server's
    // parser enforces the 10,000-game limit.
    const games = content.match(/^\s*\[Event\s/gm)?.length || content.match(/^\s*\[White\s/gm)?.length || 0
    if (content.length > INLINE_LIMIT) setFile({ name: f.name, size: f.size, text: content, games })
    else setPgn(content)
  }
  return (
    <div className="appear space-y-6">
      <PageHeader title="Import PGN" subtitle="Review games from anywhere: paste PGN text or upload a .pgn file (multiple games supported). Games whose players match a connected account count toward its insights." />
      <Card>
        <div className="space-y-4">
          <label className="flex cursor-pointer items-center justify-center gap-3 rounded-lg border border-dashed border-line-strong bg-subtle px-4 py-6 text-sm text-fg-2 transition-colors hover:border-fg-3 hover:text-fg focus-within:border-accent-ink focus-within:text-fg"
            onDragOver={e => e.preventDefault()} onDrop={e => { e.preventDefault(); onFile(e.dataTransfer.files[0]) }}>
            <FileUp className="h-5 w-5" /> Drop a .pgn file here or click to choose
            <input type="file" accept=".pgn,.txt,application/x-chess-pgn" className="sr-only" onChange={e => { const f = e.currentTarget.files?.[0]; e.currentTarget.value = ''; void onFile(f) }} />
          </label>
          {fileError && <ErrorNote error={fileError} />}
          {file ? (
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-line bg-surface p-3 text-[13px]">
              <span className="min-w-0 truncate"><b className="font-semibold">{file.name}</b> · {(file.size / 1_000_000).toFixed(1)} MB{file.games ? ` · about ${file.games.toLocaleString()} games` : ''}{file.games > MAX_GAMES ? ` (at most ${MAX_GAMES.toLocaleString()} are imported at once)` : ''}</span>
              <Button size="sm" variant="ghost" onClick={() => { setFile(null); imp.reset() }}>Clear</Button>
            </div>
          ) : <textarea value={pgn} onChange={e => setPgn(e.target.value)} disabled={reading} rows={12} spellCheck={false} aria-label="PGN text"
            placeholder={'[Event "Casual game"]\n[White "Me"]\n[Black "Friend"]\n\n1. e4 e5 2. Nf3 Nc6 ...'}
            className="w-full rounded-md border border-line bg-surface p-3 font-mono text-[13px] text-fg outline-none transition-colors placeholder:text-fg-3 hover:border-line-strong focus:border-accent-ink" />}
          <Switch checked={analyze} onChange={setAnalyze} label="Analyse immediately" description="Imported games jump to the front of the analysis queue." />
          <Button variant="primary" onClick={() => imp.mutate()} loading={reading || imp.isPending} disabled={!text.trim()}><Upload className="h-4 w-4" />Import</Button>
          {imp.error && <ErrorNote error={imp.error} />}
          {imp.data && (
            <div className="rounded-lg border border-accent/40 bg-accent/5 p-4 text-sm">
              <p><b>{imp.data.imported}</b> new game(s) imported{imp.data.existing ? `, ${imp.data.existing} already existed` : ''}.</p>
              {imp.data.errors.length > 0 && <p className="mt-1 text-bad">{imp.data.errors.length} game(s) skipped: {imp.data.errors.slice(0, 3).map(e => e.error).join('; ')}</p>}
              {imp.data.unread_after != null && <p className="mt-1 text-bad">Reading stopped after game {imp.data.unread_after.toLocaleString()}: the PGN after it is malformed, so nothing after that point was imported.</p>}
              <div className="mt-2 flex flex-wrap gap-2">
                {imp.data.game_ids.slice(0, 20).map(id => <Link key={id} to={`/games/${id}`} className="rounded-md bg-hover px-2 py-1 text-xs text-accent hover:underline">Game #{id}</Link>)}
              </div>
            </div>
          )}
        </div>
      </Card>
    </div>
  )
}
