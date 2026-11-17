import type { Config } from '@lichess-org/chessground/config'
import type { DrawShape } from '@lichess-org/chessground/draw'
import type { Key } from '@lichess-org/chessground/types'
import { Chess, type Square } from 'chess.js'
import clsx from 'clsx'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { legalDests } from '../lib/chess'
import { useAppearance } from '../lib/theme'
import { useChessground } from '../lib/useChessground'
import PromotionPicker, { type Promotion } from './PromotionPicker'

export interface BoardMove { from: string; to: string; promotion?: Promotion }
export interface BoardProps {
  fen: string
  orientation: 'white' | 'black'
  lastMove?: [string, string] | null
  shapes?: DrawShape[]
  interactive?: boolean
  onMove?: (move: BoardMove) => boolean | Promise<boolean>
  coordinates?: boolean
  className?: string
  mini?: boolean
  /** Compose nearby controls with Board's interaction state without duplicating promotion state. */
  children?: (board: ReactNode, promoting: boolean) => ReactNode
}

export default function Board({ fen, orientation, lastMove, shapes, interactive, onMove, coordinates = true, className, mini, children }: BoardProps) {
  const { board } = useAppearance()
  const [promotion, setPromotion] = useState<{ from: string; to: string; fen: string } | null>(null)
  const identity = `${fen}:${orientation}:${!!interactive}`
  const [previousPosition, setPreviousPosition] = useState(identity)
  if (previousPosition !== identity) { setPreviousPosition(identity); setPromotion(null) }
  const pending = promotion?.fen === fen ? promotion : null
  const playable = !!interactive && !pending
  const latest = useRef({ fen, onMove, interactive })
  useLayoutEffect(() => { latest.current = { fen, onMove, interactive } }, [fen, onMove, interactive])
  const reset = useRef<() => void>(() => {})
  const busy = useRef(false)
  // Legal move generation is substantially more expensive than updating an arrow.
  // Keep it tied to the position, including when a rejected move is restored.
  const position = useMemo(() => { try { return new Chess(fen) } catch { return null } }, [fen])
  const dests = useMemo(() => playable ? legalDests(fen) : new Map<Key, Key[]>(), [fen, playable])
  const submit = useCallback(async (move: BoardMove) => {
    if (busy.current || !latest.current.interactive || !latest.current.onMove) { reset.current(); return }
    busy.current = true
    try { if (!await latest.current.onMove(move)) reset.current() }
    catch { reset.current() }
    finally { busy.current = false }
  }, [])
  const after = useCallback((from: Key, to: Key) => {
    const current = latest.current
    let pawn = false
    try { pawn = new Chess(current.fen).get(from as Square)?.type === 'p' } catch { /* invalid position */ }
    if (pawn && (to[1] === '8' || to[1] === '1')) setPromotion({ from, to, fen: current.fen })
    else void submit({ from, to })
  }, [submit])
  const first = lastMove?.[0], second = lastMove?.[1]
  const config = useMemo((): Config => ({
    fen, orientation,
    turnColor: fen.split(' ')[1] === 'b' ? 'black' : 'white',
    lastMove: first && second ? [first as Key, second as Key] : undefined,
    check: position?.inCheck() ?? false,
    coordinates: coordinates && !mini,
    viewOnly: !playable,
    animation: { enabled: !mini, duration: 160 },
    highlight: { lastMove: true, check: true },
    movable: playable
      ? { free: false, color: fen.split(' ')[1] === 'b' ? 'black' : 'white', dests: new Map(dests), showDests: true, events: { after } }
      : { free: false, color: undefined, dests: new Map() },
    premovable: { enabled: false },
    draggable: { enabled: playable, showGhost: true },
    selectable: { enabled: playable },
    drawable: { enabled: !mini, visible: true, defaultSnapToValidMove: true },
  }), [fen, orientation, first, second, position, coordinates, mini, playable, dests, after])
  const { el, restore, measure } = useChessground(config, shapes)
  useLayoutEffect(() => { reset.current = restore }, [restore])
  const cancel = useCallback(() => { setPromotion(null); restore() }, [restore])
  useEffect(() => {
    if (!pending) return
    const onKey = (event: KeyboardEvent) => {
      // A promotion choice owns the page's move shortcuts until it is picked or cancelled, but not
      // browser shortcuts or typing in a field elsewhere (the command palette, a search box).
      if (event.key !== 'Escape' && (event.metaKey || event.ctrlKey || event.altKey
        || (event.target instanceof Element && event.target.closest('input, textarea, select, [contenteditable="true"]')))) return
      if (['Escape', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'Backspace', 'f', 'n'].includes(event.key)) {
        event.preventDefault(); event.stopImmediatePropagation()
        if (event.key === 'Escape') cancel()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [pending, cancel])
  const content = (
    <div className="relative">
      <div data-board={board} className={clsx('board', `orientation-${orientation}`, mini && 'mini', className)} onPointerDownCapture={measure}>
        <div ref={el} />
      </div>
      {pending && <>{!children && <span className="sr-only" role="status">Choose a piece to promote to.</span>}
        <PromotionPicker onCancel={cancel} onPick={piece => { setPromotion(null); void submit({ from: pending.from, to: pending.to, promotion: piece }) }} />
      </>}
    </div>
  )
  return children ? children(content, !!pending) : content
}
