import { Chessground } from '@lichess-org/chessground'
import type { Api } from '@lichess-org/chessground/api'
import type { Config } from '@lichess-org/chessground/config'
import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { Chess } from 'chess.js'
import clsx from 'clsx'
import { useEffect, useMemo, useRef } from 'react'
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
  /** Change to put the board back to `fen`, e.g. to undo a rejected move. */
  nonce?: number
}

function turnColor(fen: string): 'white' | 'black' {
  return fen.split(' ')[1] === 'b' ? 'black' : 'white'
}

function inCheck(fen: string): boolean {
  try { return new Chess(fen).inCheck() } catch { return false }
}

export default function Board({ fen, orientation, lastMove, shapes, interactive, onMove, coordinates = true, className, mini, nonce }: BoardProps) {
  const { board } = useAppearance()
  const el = useRef<HTMLDivElement>(null)
  const api = useRef<Api | null>(null)
  const onMoveRef = useRef(onMove)
  onMoveRef.current = onMove
  // Legal move generation is substantially more expensive than updating an arrow.
  // Keep it tied to the position, including when a rejected move is reset by nonce.
  const check = useMemo(() => inCheck(fen), [fen])
  const dests = useMemo(() => interactive ? legalDests(fen) : new Map<Key, Key[]>(), [fen, interactive])

  const config = (): Config => ({
    fen,
    orientation,
    turnColor: turnColor(fen),
    lastMove: lastMove ? (lastMove as Key[]) : undefined,
    check,
    coordinates: coordinates && !mini,
    viewOnly: !interactive,
    animation: { enabled: !mini, duration: 160 },
    highlight: { lastMove: true, check: true },
    movable: interactive
      ? { free: false, color: turnColor(fen), dests: new Map(dests), showDests: true,
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fen, orientation, lastMove?.[0], lastMove?.[1], interactive, coordinates, mini, nonce])

  useEffect(() => {
    api.current?.setAutoShapes(shapes ?? [])
    // Compare shape content: callers may create an equivalent array on each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(shapes ?? [])])

  return (
    <div data-board={board} className={clsx('board', `orientation-${orientation}`, mini && 'mini', className)}
      // Chessground caches the board's screen position and only refreshes it on scroll or resize.
      // Content above the board can move it without either (a status line, a wrapping header row),
      // and then every click lands on the wrong square. Measure again on each press (#29).
      onPointerDownCapture={() => api.current?.state.dom.bounds.clear()}>
      <div ref={el} />
    </div>
  )
}
