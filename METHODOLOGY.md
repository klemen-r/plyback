# Analysis methodology

Every number Chess Insight shows is produced by a documented, reproducible rule. This page describes them.
None of this replicates Chess.com's proprietary Game Review; labels and accuracy figures will differ from theirs.

## 1. Engine evaluation

- Engine: the latest stable Stockfish, installed from the official GitHub release.
- Each position of a game is analysed to a fixed depth (default 16; presets 12 / 16 / 20; configurable 6–30)
  with MultiPV 2 by default, and a per-position time cap (default 5 s) so pathological positions can't stall the queue.
- The evaluation *after* a move is the engine's evaluation of the next position (searched in its own right), which is
  more reliable than reading the played move out of the previous search.
- Evaluations are stored from White's point of view; mates are encoded as ±(100000 − moves-to-mate).
- Opening positions (first 30 plies) are cached across games; each job checkpoints its evaluations so an interrupted
  analysis resumes where it stopped.

## 2. Winning chances

Centipawns are converted to winning chances with the logistic model fitted by Lichess on real games:

    win% = 50 + 50 · (2 / (1 + exp(−0.00368208 · cp)) − 1)        cp capped to ±1000; mate = 100% / 0%

The **cost** of a move ("expected-points loss") is

    loss = win%(best move, mover's view) − win%(position after the played move, mover's view)    (≥ 0)

and is 0 when the played move is the engine's first choice. Measuring in winning chances means that going from +9 to
+5 (still completely winning) costs almost nothing, while going from +1 to −1 costs a lot.

## 3. Move classifications

Evaluated in this order:

| Label | Rule |
|---|---|
| Forced | Only one legal move. |
| Book | The position after the move is in the Lichess `chess-openings` database (CC0), continuously from move 1. |
| Brilliant | The engine's best move **and** a sacrifice (below) **and** the mover's winning chances afterwards are ≥ 50% **and** the position was not already decided — the second-best move is below 80% — unless the sacrifice forces mate where the alternatives don't. |
| Great | The engine's best move **and** every alternative is ≥ 15 points worse (an "only move") **and** the mover is not lost afterwards (≥ 35%) **and** the second-best move is below 90%. Excluded: plain recaptures, captures of hanging material (SEE ≥ 2), checkmates, queen promotions, and check evasions with ≤ 2 legal moves. |
| Best | The engine's first choice. |
| Excellent | loss < 2 points |
| Good | loss < 5 |
| Inaccuracy | loss < 10 |
| Mistake | loss < 20 |
| Blunder | loss ≥ 20 |
| Miss | A Mistake/Blunder made immediately after the opponent's error — the mover's best-move chances rose ≥ 10 points over the previous position — where the move does not leave the mover worse than before the opponent's error (within 5 points). It "throws the gift back". |

**Sacrifice:** after the move, the opponent can win at least 2 points of material (static exchange evaluation with
real legal moves, so pins and x-rays are respected) more than the move itself captured. Only the moved piece, or a
piece that becomes capturable *because of* the move, counts — pieces that were already en prise do not.
If, after the opponent takes, the mover can immediately win back at least as much (an in-between move or
desperado), it is not a sacrifice.

## 4. Accuracy and centipawn loss

- **Move accuracy** (Lichess): `103.1668 · exp(−0.04354 · drop) − 3.1669 + 1`, clamped to 0–100, where `drop` is the
  loss in winning chances.
- **Game accuracy** (Lichess): the average of a volatility-weighted mean (weights = standard deviation of winning
  chances in a sliding window, clamped 0.5–12) and the harmonic mean of the move accuracies. Sharp positions weigh more,
  and one blunder isn't hidden by many easy moves.
- **ACPL:** mean centipawn loss of non-book moves, evaluations capped at ±1000.

## 5. Explanations and tactical tags

Computed for inaccuracies, mistakes, misses and blunders:

| Tag | Rule |
|---|---|
| allowed_mate | After the move the opponent has a forced mate (and didn't before). `back_rank` if the final mate is delivered by a rook/queen along the king's back rank. |
| missed_mate | A mate in ≤ 3 was available and the move doesn't keep a mate. |
| hanging_piece | The opponent's best reply captures one of the mover's pieces and the line loses ≥ 2 points of material. |
| allows_fork | The opponent's best reply attacks two or more valuable targets (king, higher-value or undefended pieces) and the line loses ≥ 2 points. |
| loses_material | Material balance after the opponent's best line (up to 6 plies, extended to finish exchanges) is ≥ 2 points lower. |
| missed_material / missed_fork | The best line would have won ≥ 2 points of material (with a fork as the first move). |
| threat_ignored | Null-move test: had the mover passed, the opponent would gain ≥ 15 points — and the opponent's refutation of the played move executes that same threat. |
| self_inflicted | A mistake/blunder with no pre-existing threat and no missed punish. |
| tactical / positional | Tactical if any concrete material/mate tag applies, otherwise positional. |
| time_pressure | The mover had ≤ 10% of the base time (min. 10 s) left. |

**Game phases:** endgame when the non-pawn material of both sides is ≤ 26 points (or queens are off and ≤ 32);
opening during the first 10 moves or while still in book; otherwise middlegame.

## 6. Account insights

- Result statistics (score, openings, ratings, sessions) use all games in the selection; engine statistics only use
  analysed games, and the coverage is always shown.
- **Confidence labels** are based on sample size (e.g. fewer than 5 occurrences = "not enough data").
  Score ranges use Wilson 95% intervals; an opening is flagged as a weakness/strength only if its interval excludes
  your overall score, or it differs by ≥ 15 points over ≥ 8 games.
- **Opponent baseline:** the same metrics are computed for your opponents in the same games. Because Chess.com pairs
  players of similar strength, this is a fair like-for-like comparison.
- **Priorities:** for each area (piece safety, threats, king safety, forks, missed tactics, conversion, endgame,
  opening, time pressure, positional play) we sum the winning chances lost per game in matching errors, weight by how
  that compares with your opponents (×0.6–1.6), drop areas with insufficient data, and rank. Areas overlap — one
  blunder can both hang a piece and ignore a threat.
- **Recurring mistakes:** identical positions (same placement, side to move, castling and en passant) reached in two or
  more different games where you lost ≥ 5 points.
- **Tilt:** score in games started within 30 minutes of a loss vs. your overall score; flagged only when the Wilson
  interval lies entirely below the baseline.
- **Performance rating:** average opponent rating + 400 × (wins − losses) / games (linear approximation).

## 7. Known limitations

- Moderate-depth engine analysis can misjudge very deep tactics; re-analyse a game deeper from the review page.
- Brilliant/Great are heuristics; they will not always match human judgement or Chess.com's labels.
- Premoves, mouse slips and lag are invisible to us. Time statistics require clock data (live games include it).
- Chess.com's public API can lag a few minutes behind the website for brand-new games.
