import { useMutation, useQueryClient } from '@tanstack/react-query'
import { FileUp, Upload } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { PageHeader, Button, Card, ErrorNote, Switch } from '../components/ui'
import { api } from '../lib/api'

export default function ImportPgn() {
  const qc = useQueryClient()
  const [pgn, setPgn] = useState('')
  const [fileError, setFileError] = useState<string | null>(null)
  const [analyze, setAnalyze] = useState(true)
  const imp = useMutation({ mutationFn: () => api.importPgn(pgn, analyze), onSuccess: () => {
    for (const key of ['games', 'accounts', 'insights', 'status', 'openings']) qc.invalidateQueries({ queryKey: [key] })
  } })
  const onFile = async (f: File | undefined) => {
    if (!f) return
    if (f.size > 25 * 1024 * 1024) { setFileError('Choose a PGN file smaller than 25 MB.'); return }
    try { setPgn(await f.text()); setFileError(null); imp.reset() }
    catch { setFileError('The selected file could not be read. Try choosing it again.') }
  }
  return (
    <div className="appear space-y-6">
      <PageHeader title="Import PGN" subtitle="Review games from anywhere: paste PGN text or upload a .pgn file (multiple games supported). Games whose players match a connected account count toward its insights." />
      <Card>
        <div className="space-y-4">
          <label className="flex cursor-pointer items-center justify-center gap-3 rounded-lg border border-dashed border-line-strong bg-subtle px-4 py-6 text-sm text-fg-2 transition-colors hover:border-fg-3 hover:text-fg"
            onDragOver={e => e.preventDefault()} onDrop={e => { e.preventDefault(); onFile(e.dataTransfer.files[0]) }}>
            <FileUp className="h-5 w-5" /> Drop a .pgn file here or click to choose
            <input type="file" accept=".pgn,.txt,application/x-chess-pgn" className="hidden" onChange={e => onFile(e.target.files?.[0])} />
          </label>
          {fileError && <ErrorNote error={fileError} />}
          <textarea value={pgn} onChange={e => setPgn(e.target.value)} rows={12} spellCheck={false} aria-label="PGN text"
            placeholder={'[Event "Casual game"]\n[White "Me"]\n[Black "Friend"]\n\n1. e4 e5 2. Nf3 Nc6 ...'}
            className="w-full rounded-md border border-line bg-surface p-3 font-mono text-[13px] text-fg outline-none transition-colors placeholder:text-fg-3 hover:border-line-strong focus:border-accent-ink" />
          <Switch checked={analyze} onChange={setAnalyze} label="Analyse immediately" description="Imported games jump to the front of the analysis queue." />
          <Button variant="primary" onClick={() => imp.mutate()} loading={imp.isPending} disabled={!pgn.trim()}><Upload className="h-4 w-4" />Import</Button>
          {imp.error && <ErrorNote error={imp.error} />}
          {imp.data && (
            <div className="rounded-lg border border-accent/40 bg-accent/5 p-4 text-sm">
              <p><b>{imp.data.imported}</b> new game(s) imported{imp.data.existing ? `, ${imp.data.existing} already existed` : ''}.</p>
              {imp.data.errors.length > 0 && <p className="mt-1 text-bad">{imp.data.errors.length} game(s) skipped: {imp.data.errors.slice(0, 3).map(e => e.error).join('; ')}</p>}
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
