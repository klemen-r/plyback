import { PageHeader } from '../components/ui'
import type { ReactNode } from 'react'
import ClassIcon from '../components/ClassIcon'
import { Card } from '../components/ui'
import { CLASS_META, CLASS_ORDER } from '../lib/classification'
import type { Classification } from '../lib/types'

const RULES: Record<Classification, string> = {
  brilliant: 'The engine’s best move offers a piece or exchange worth at least 2 points, without winning the material back immediately or within the engine’s next four moves, and keeps your estimated score at 50% or more. Best and second-best lines must reach the same depth of at least 10. The second-best score stays below 80%, unless the best line forces mate and the second-best does not.',
  great: 'The engine’s best move scores at least 15 percentage points above the second-best, keeps your estimated score at 35% or more, and the second-best stays below 90%. Both lines must reach the same depth of at least 10. Plain recaptures, grabbing free material, checkmates, queen promotions and check evasions with only one or two legal moves are excluded.',
  best: 'The engine’s first choice.',
  excellent: 'Loses at most 2 percentage points of estimated score.',
  good: 'Loses more than 2 and at most 5 points.',
  book: 'The position after the move appears in the opening database (Lichess chess-openings, CC0), continuously from the standard starting position. Custom starting positions and Chess960 are excluded.',
  forced: 'The only legal move.',
  inaccuracy: 'Loses more than 5 and at most 10 points.',
  mistake: 'Loses more than 10 and at most 20 points.',
  miss: 'A mistake or blunder immediately after your opponent lost at least 10 points and improved your best available score by at least 10 points. Your move throws the gift back while retaining your earlier score within 5 points.',
  blunder: 'Loses more than 20 points.',
}

function Block({ title, children }: { title: string; children: ReactNode }) {
  return <Card title={title}><div className="prose-sm space-y-3 text-sm leading-relaxed text-fg/90">{children}</div></Card>
}

export default function Methodology() {
  return (
    <div className="appear max-w-4xl space-y-6">
      <PageHeader title="How the analysis works" subtitle="Plyback’s review scores follow documented rules. Nothing here replicates Chess.com’s proprietary algorithms." />
      <Block title="Engine">
        <p>Stockfish runs locally, aiming for your chosen depth (default 16) with two candidate lines by default. Each search has a time limit (default 5 seconds), so it may finish at a lower depth. We use the deepest completed set of candidate scores at a common depth.</p>
        <p>The best move and the played move are compared from the same position and move history, under the same search limits. If the played move is missing from the initial lines, Stockfish searches the two candidates together. The evaluation graph and opponent’s reply are searched separately; those scores do not determine the move’s loss.</p>
        <p>Completed searches are saved with your database and reused when the game history, engine and settings match. An interrupted review restarts and can reuse that saved work; a search interrupted before completion must be repeated.</p>
      </Block>
      <Block title="Winning chances">
        <p>Centipawn evaluations are converted with the public Lichess logistic curve. We use it as an estimated expected-points score, shown as a percentage; it is not a calibrated prediction of your personal win, draw or loss probabilities:</p>
        <pre className="overflow-x-auto rounded-lg bg-subtle p-3 font-mono text-xs">win% = 50 + 50 × (2 / (1 + e^(−0.00368208 × cp)) − 1)      (cp capped at ±1000, mate = 0% / 100%)</pre>
        <p>We take the best move’s estimated score minus the played move’s score, both from the mover’s perspective, and clamp negative loss to zero. Black’s score is the complement of White’s. A loss of 0.02 means two percentage points. The model gives a smaller penalty to +9 → +5 than to +1 → −1, and retains forced mates as exactly 0% or 100%.</p>
      </Block>
      <Block title="Move classifications">
        <p>Loss gives the base rating. Special labels take precedence in this order: Forced → Book → Brilliant → Great → Miss → base rating. Brilliant and Great require a second candidate at the same completed depth; with only one line, the base rating remains.</p>
        <div className="space-y-2.5">
          {CLASS_ORDER.concat(['forced']).map(c => (
            <div key={c} className="flex gap-3"><ClassIcon c={c} size={22} /><div><b style={{ color: CLASS_META[c].color }}>{CLASS_META[c].label}</b> — {RULES[c]}</div></div>
          ))}
        </div>
      </Block>
      <Block title="Accuracy and centipawn loss">
        <p>Move accuracy follows the Lichess formula <code className="rounded bg-subtle px-1">103.17 × e^(−0.0435 × drop) − 3.17</code> (+1 to allow for analysis noise), clamped to 0–100. The drop is the loss above in percentage points. Game accuracy is the average of a volatility-weighted mean and a harmonic mean of move accuracies, so mistakes in sharp positions weigh more and a single blunder isn’t hidden by many easy moves.</p>
        <p>Average centipawn loss (ACPL) uses the same best/played comparison, averaged across non-book moves with evaluations capped at ±10 pawns. Chess.com’s own accuracy numbers (when Chess.com has reviewed a game) are shown separately and will differ.</p>
      </Block>
      <Block title="Explanations and tactical tags">
        <p>Explanations use mate distances, material counted over a few moves of the engine’s lines, legal exchange checks for sacrifices, piece attacks for forks, and a <i>null-move test</i> for threats. That test lets the opponent move as though you had passed: if they gain at least 15 estimated points and their refutation of your move matches the threat, your move ignored an existing threat.</p>
        <p>Errors with a detected material, mating or threat consequence are labelled <i>tactical</i>; the rest are <i>positional</i>. Game phases: endgame = little material left (≤26 points of pieces, or queens off and ≤32); otherwise opening = first 10 moves or still in book; otherwise middlegame.</p>
      </Block>
      <Block title="Account insights">
        <p>Result statistics use every game in the selection; engine statistics only use analysed games. Every pattern shows its sample size and a confidence label, and score ranges use Wilson 95% intervals. Openings are only flagged as a weakness or strength when the difference is large relative to the sample.</p>
        <p>Where possible you are compared with <b>your opponents in the same games</b>: because Chess.com pairs players of similar rating, this is a fair like-for-like baseline. Improvement priorities are ranked by winning chances lost per game in each area, weighted by how you compare with those opponents. Categories can overlap (one blunder can both hang a piece and ignore a threat).</p>
      </Block>
      <Block title="Practice">
        <p>Practice uses your own moves reviewed as a blunder, mistake or miss in standard chess, but only where the engine’s best move was at least <b>5 points of winning chance</b> better than its second choice. That makes the best move the one fair answer, so your answer is checked against the stored review without a new engine search. Positions where two moves are nearly equal are left out.</p>
        <p>A wrong move is put back so you can try again or ask for a hint, but only the first try counts: a correct first try brings the position back after 1, 3, 7, 16, 35, then 80 days, and a wrong first try or <i>Show answer</i> brings it back after 10 minutes. Four correct answers in a row count as mastered.</p>
        <p>With <i>Play out the line</i> your opponent replies with the engine line and you continue for up to three moves. Later in a line more than one move can be good, so a different move is checked with the live engine and accepted when it is within 5 points of winning chance of the best one. An accepted alternative ends the puzzle because the stored line no longer applies. If the engine check fails, the move is not counted as wrong.</p>
        <p><i>Today’s games</i> and the 7- and 30-day ranges go through every position from those games in random order. Filter by mistake type or game phase to focus on opening, middlegame or endgame positions; progress counts use the same selection.</p>
      </Block>
      <Block title="Openings">
        <p>The Openings page shows your reviewed games for one colour as a move tree over the first 8 moves, with an option to explore the first 15. The depth and current line are saved in the URL. Moves stay in the order they were played (transpositions stay apart). Each move shows your score afterwards, how often it was book, where games left book and, for your own moves, how often it was an error.</p>
        <p>Positions are named from the same Lichess opening dataset used in reviews, and each line links to your latest games through it. <i>Moves to fix</i> lists your moves from at least three games that were an error in half of them or more, or after which you scored 35% or less over four or more games. These are patterns in your games, not proof that a move is unsound.</p>
      </Block>
      <Block title="Limitations">
        <ul className="list-disc space-y-1 pl-5">
          <li>Engine evaluations at moderate depth are not perfect; very deep tactics can be misjudged. Re-analyse a game deeper from the review page if in doubt.</li>
          <li>Time limits can reduce the completed depth. Extra candidate and threat searches take time; saved results help repeated reviews, but a new game still needs engine work.</li>
          <li>Brilliant/Great definitions are our own, documented heuristics — they will not always match Chess.com’s labels.</li>
          <li>Estimated scores and tactical tags have limits. Tags inspect a few engine lines; an absent tag does not prove there is no tactic.</li>
          <li>We cannot know premoves, mouse slips, or connection issues. Time statistics need clock data, which Chess.com includes for live games.</li>
          <li>Chess.com’s public API can lag by a few minutes for new games; polling backs off automatically on rate limits or errors.</li>
        </ul>
      </Block>
    </div>
  )
}
