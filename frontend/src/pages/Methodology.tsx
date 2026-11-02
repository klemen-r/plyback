import { PageHeader } from '../components/ui'
import type { ReactNode } from 'react'
import ClassIcon from '../components/ClassIcon'
import { Card } from '../components/ui'
import { CLASS_META, CLASS_ORDER } from '../lib/classification'
import type { Classification } from '../lib/types'

const RULES: Record<Classification, string> = {
  brilliant: 'The engine’s best move that sacrifices at least 2 points of material — a piece, or the exchange — where you are at least equal afterwards and the position was not already decided (the next-best move stays below 80% winning chances), and the opponent cannot take the material without immediately losing as much back, unless the sacrifice forces mate where the alternatives don’t.',
  great: 'The engine’s top move where the second-best move is at least 15 percentage points of winning chance worse, and you are not losing afterwards. Plain recaptures, grabbing free material, checkmates, queen promotions and forced check evasions are excluded.',
  best: 'The engine’s first choice.',
  excellent: 'Loses less than 2 percentage points of winning chance.',
  good: 'Loses 2–5 points.',
  book: 'The position after the move appears in the opening database (Lichess chess-openings, CC0), continuously from move 1.',
  forced: 'The only legal move.',
  inaccuracy: 'Loses 5–10 points.',
  mistake: 'Loses 10–20 points.',
  miss: 'A mistake or blunder made right after your opponent erred (your chances jumped ≥10 points), which throws the gift back without leaving you worse than before their error.',
  blunder: 'Loses 20 points or more.',
}

function Block({ title, children }: { title: string; children: ReactNode }) {
  return <Card title={title}><div className="prose-sm space-y-3 text-sm leading-relaxed text-fg/90">{children}</div></Card>
}

export default function Methodology() {
  return (
    <div className="appear max-w-4xl space-y-6">
      <PageHeader title="How the analysis works" subtitle="Every number in Chess Insight comes from a documented, reproducible method. Nothing here replicates Chess.com’s proprietary algorithms." />
      <Block title="Engine">
        <p>Each position of a game is evaluated by Stockfish (the latest stable release, running locally) to a fixed depth (default 16, configurable) with at least two principal variations, so we know both the best move and how much worse the alternatives are. Opening positions are cached and reused across games; interrupted analyses resume where they stopped.</p>
      </Block>
      <Block title="Winning chances">
        <p>Centipawn evaluations are converted into winning chances with the logistic curve published by Lichess, fitted on millions of real games:</p>
        <pre className="overflow-x-auto rounded-lg bg-subtle p-3 font-mono text-xs">win% = 50 + 50 × (2 / (1 + e^(−0.00368208 × cp)) − 1)      (cp capped at ±1000, mate = 0% / 100%)</pre>
        <p>The cost of a move is the drop in the mover’s winning chances between the best move and the move played. Working in winning chances rather than raw centipawns means that dropping from +9 to +5 (still completely winning) is barely penalised, while dropping from +1 to −1 is.</p>
      </Block>
      <Block title="Move classifications">
        <div className="space-y-2.5">
          {CLASS_ORDER.concat(['forced']).map(c => (
            <div key={c} className="flex gap-3"><ClassIcon c={c} size={22} /><div><b style={{ color: CLASS_META[c].color }}>{CLASS_META[c].label}</b> — {RULES[c]}</div></div>
          ))}
        </div>
      </Block>
      <Block title="Accuracy and centipawn loss">
        <p>Move accuracy follows the Lichess formula <code className="rounded bg-subtle px-1">103.17 × e^(−0.0435 × drop) − 3.17</code> (+1 to allow for analysis noise), clamped to 0–100. Game accuracy is the average of a volatility-weighted mean and a harmonic mean of move accuracies, so mistakes in sharp positions weigh more and a single blunder isn’t hidden by many easy moves.</p>
        <p>Average centipawn loss (ACPL) is the mean loss per non-book move with evaluations capped at ±10 pawns. Chess.com’s own accuracy numbers (when Chess.com has reviewed a game) are shown separately and will differ.</p>
      </Block>
      <Block title="Explanations and tactical tags">
        <p>Explanations are built from facts we can verify: mate distances, material counted along the engine’s lines (stopping at quiet positions), static exchange evaluation for hanging pieces, piece attacks for forks, and a <i>null-move test</i> for threats — we let the opponent move twice; if that gains them ≥15 points and their refutation of your move executes the same idea, your move ignored an existing threat.</p>
        <p>Errors with a concrete material or mating consequence within a few moves are labelled <i>tactical</i>; the rest are <i>positional</i>. Game phases: opening = first 10 moves or still in book; endgame = little material left (≤26 points of pieces, or queens off and ≤32); otherwise middlegame.</p>
      </Block>
      <Block title="Account insights">
        <p>Result statistics use every game in the selection; engine statistics only use analysed games. Every pattern shows its sample size and a confidence label, and score ranges use Wilson 95% intervals. Openings are only flagged as a weakness or strength when the difference is large relative to the sample.</p>
        <p>Where possible you are compared with <b>your opponents in the same games</b>: because Chess.com pairs players of similar rating, this is a fair like-for-like baseline. Improvement priorities are ranked by winning chances lost per game in each area, weighted by how you compare with those opponents. Categories can overlap (one blunder can both hang a piece and ignore a threat).</p>
      </Block>
      <Block title="Limitations">
        <ul className="list-disc space-y-1 pl-5">
          <li>Engine evaluations at moderate depth are not perfect; very deep tactics can be misjudged. Re-analyse a game deeper from the review page if in doubt.</li>
          <li>Brilliant/Great definitions are our own, documented heuristics — they will not always match Chess.com’s labels.</li>
          <li>We cannot know premoves, mouse slips, or connection issues. Time statistics need clock data, which Chess.com includes for live games.</li>
          <li>Chess.com’s public API can lag by a few minutes for new games; polling backs off automatically on rate limits or errors.</li>
        </ul>
      </Block>
    </div>
  )
}
