import { Chessground } from '@lichess-org/chessground'
import type { Api } from '@lichess-org/chessground/api'
import type { Config } from '@lichess-org/chessground/config'
import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { Chess } from 'chess.js'
import clsx from 'clsx'
import { useEffect, useRef } from 'react'
import { legalDests } from '../lib/chess'
import { useAppearance } from '../lib/theme'

export interface BoardProps {
  fen: string
  orientation: 'white' | 'black'
  lastMove?: [string, string] | null
  shapes?: DrawShape[]
  interactive?: boolean
  onMove?: (orig: string, dest: string) => void
  coordinates?: boolean
  className?: string
  mini?: boolean
}

function turnColor(fen: string): 'white' | 'black' {
  return fen.split(' ')[1] === 'b' ? 'black' : 'white'
}

function inCheck(fen: string): boolean {
  try { return new Chess(fen).inCheck() } catch { return false }
}

export default function Board({ fen, orientation, lastMove, shapes, interactive, onMove, coordinates = true, className, mini }: BoardProps) {
  const { board } = useAppearance()
  const el = useRef<HTMLDivElement>(null)
  const api = useRef<Api | null>(null)
  const onMoveRef = useRef(onMove)
  onMoveRef.current = onMove

  const config = (): Config => ({
    fen,
    orientation,
    turnColor: turnColor(fen),
    lastMove: lastMove ? (lastMove as Key[]) : undefined,
    check: inCheck(fen),
    coordinates: coordinates && !mini,
    viewOnly: !interactive,
    animation: { enabled: !mini, duration: 160 },
    highlight: { lastMove: true, check: true },
    movable: interactive
      ? { free: false, color: turnColor(fen), dests: legalDests(fen), showDests: true,
          events: { after: (o, d) => onMoveRef.current?.(o, d) } }
      : { free: false, color: undefined, dests: new Map() },
    premovable: { enabled: false },
    draggable: { enabled: !!interactive, showGhost: true },
    selectable: { enabled: !!interactive },
    drawable: { enabled: !mini, visible: true, autoShapes: shapes ?? [], defaultSnapToValidMove: true },
  })

  useEffect(() => {
    if (!el.current) return
    api.current = Chessground(el.current, config())
    const ro = new ResizeObserver(() => api.current?.redrawAll())
    ro.observe(el.current)
    return () => {
      ro.disconnect()
      api.current?.destroy()
      api.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!api.current) return
    api.current.set(config())
    api.current.setAutoShapes(shapes ?? [])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fen, orientation, lastMove?.[0], lastMove?.[1], interactive, JSON.stringify(shapes ?? []), coordinates])

  return (
    <div data-board={board} className={clsx('board', `orientation-${orientation}`, mini && 'mini', className)}>
      <div ref={el} />
    </div>
  )
}
