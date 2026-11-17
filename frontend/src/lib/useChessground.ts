import { Chessground } from '@lichess-org/chessground'
import type { Api } from '@lichess-org/chessground/api'
import type { Config } from '@lichess-org/chessground/config'
import type { DrawShape } from '@lichess-org/chessground/draw'
import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'

/** Own the board and its library-specific event and geometry lifecycle. */
export function useChessground(config: Config, shapes: DrawShape[] = []) {
  const el = useRef<HTMLDivElement>(null)
  const api = useRef<Api | null>(null)
  const latest = useRef(config)
  useLayoutEffect(() => { latest.current = config }, [config])
  const interactive = !config.viewOnly
  const previousShapes = useRef('')
  // Chessground binds its mouse and touch handlers only when it builds a board that is not view-only,
  // and it rebuilds the board on a flip or a resize. Turning `viewOnly` off later does not bind them,
  // so a board rebuilt while locked (a flip to the next position, a resize between moves) would
  // ignore every move. Build a new board whenever it switches between locked and playable.
  useEffect(() => {
    if (!el.current) return
    api.current = Chessground(el.current, latest.current)
    previousShapes.current = ''
    // A board being removed or hidden reports 0×0. Redrawing then divides by zero and writes
    // NaN arrow coordinates, so wait until it has a size again.
    const ro = new ResizeObserver(([entry]) => {
      if (entry && (entry.contentRect.width === 0 || entry.contentRect.height === 0)) return
      api.current?.redrawAll()
    })
    ro.observe(el.current)
    return () => { ro.disconnect(); api.current?.destroy(); api.current = null }
  }, [interactive])
  const restore = useCallback(() => { api.current?.set(latest.current) }, [])
  useEffect(restore, [config, restore])
  // Compare shape content: callers may create an equivalent array on each render.
  useEffect(() => {
    const serialized = JSON.stringify(shapes)
    if (previousShapes.current !== serialized) {
      api.current?.setAutoShapes(shapes)
      previousShapes.current = serialized
    }
  })
  // Chessground caches the board's screen position and only refreshes it on scroll or resize.
  // Content above the board can move it without either (a status line, a wrapping header row),
  // and then every click lands on the wrong square. Measure again on each press.
  const measure = useCallback(() => api.current?.state.dom.bounds.clear(), [])
  return { el, restore, measure }
}
