import { Button } from './ui'

export type Promotion = 'q' | 'r' | 'b' | 'n'

const PIECES = [['q', 'Queen'], ['r', 'Rook'], ['b', 'Bishop'], ['n', 'Knight']] as const

/** Choice of piece for a pawn reaching the last rank. Place it in a `relative` box around the board. */
export default function PromotionPicker({ onPick, onCancel }: { onPick: (piece: Promotion) => void; onCancel: () => void }) {
  return (
    // Over the board, so the picker is in view on a phone too.
    <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/25 p-4 backdrop-blur-[2px]" onClick={onCancel}>
      <div className="flex flex-wrap items-center justify-center gap-2 rounded-lg border border-line bg-surface/90 p-3 shadow-lg backdrop-blur-xl backdrop-saturate-150"
        role="group" aria-label="Promote pawn" onClick={e => e.stopPropagation()}>
        <span className="w-full text-center text-[13px] text-fg-2">Promote to</span>
        {PIECES.map(([p, label]) => <Button key={p} onClick={() => onPick(p)}>{label}</Button>)}
        <Button variant="ghost" onClick={onCancel}>Cancel</Button>
      </div>
    </div>
  )
}
