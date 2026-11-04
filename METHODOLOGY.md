# Analysis methodology

Every review score Plyback shows is produced by a documented rule. This page describes those rules.
None of this replicates Chess.com's proprietary Game Review; labels and accuracy figures will differ from theirs.

## 1. Engine evaluation

- Engine: the locally installed Stockfish. Its reported name/version is stored with the review; a worker reuses one
  engine process across searches.
- Each position is searched to a target depth (default 16; presets 12 / 16 / 20; configurable 6–30), with MultiPV 2
  by default and a time cap per search (default 5 s). A capped search can finish below the requested depth. Only the
  deepest complete set of exact scores at a common depth is accepted; partial iterations and score bounds are discarded.
- The best move and the played move are compared from the **same position, with the same move history, search limits
  and completed depth**. If the played move is absent from the initial MultiPV lines, an additional two-candidate
  search evaluates it together with the initial best move. Search noise can reorder the pair; negative loss is clamped to zero.
- A move's `eval_before` and `eval_after` are these comparable best/played scores. The evaluation graph and opponent's
  reply use separate searches of the actual positions. Their scores can differ and are not substituted into move loss.
- Evaluations are stored from White's point of view. Mate scores retain their signed distance in the lines and move
  metadata, as well as the compatible encoding ±(100000 − moves-to-mate). Forced mates map to exactly 0 or 1 in the model.
- Standard chess, setup FENs and Chess960 are supported. Full move history preserves repetition context; automatic draws
  are handled separately from claimable draws. Unsupported variants and illegal mainlines are rejected.
- Completed searches are saved in the current database's durable position cache, which is periodically trimmed to
  20,000 entries. Keys include engine identity, analysis version, threads/hash, full history, variant, limits and candidate
  restrictions; results retain their achieved depth. On restart, a job revisits the game and reuses available matching
  entries. Busy databases can skip optional cache reads/writes. This is not a Python checkpoint file or a guarantee that
  every interrupted search is retained.

## 2. Winning chances

Centipawns are converted with the public Lichess logistic curve. Plyback uses it as an **estimated expected-points
model**, not a calibrated win/draw/loss forecast for a particular player or time control:

    E_white(cp) = 1 / (1 + exp(−0.00368208 · clamp(cp, −1000, 1000)))
    E_black = 1 − E_white                         forced mate = 1 or 0
    displayed chance = 100 · E

The **cost** of a move ("expected-points loss") is

    loss = max(0, E(best move, mover's view) − E(played move, mover's view))

Both scores come from the same root search described above. The engine's first choice has zero loss. A loss of 0.02
is two percentage points in the interface. This model assigns a smaller cost to +9 → +5 than to +1 → −1.
The model identifier (`lichess-logistic-cp-v1`) and comparison depth/source are stored with the review so a future
calibrated model can be distinguished from this heuristic.

## 3. Move classifications

The loss determines a base rating. Overlays take precedence in this order: **Forced → Book → Brilliant → Great → Miss
→ base rating**. The original base rating is retained in move metadata. Missing evidence leaves the base rating intact.

| Label | Rule |
|---|---|
| Forced | Only one legal move. |
| Book | The position after the move is in the Lichess `chess-openings` database (CC0), continuously from the standard starting position. Custom starting positions and Chess960 are not labelled Book. |
| Brilliant | The engine's best move **and** a sacrifice (below) **and** the mover's expected points afterwards are ≥ 50%. Best and second-best lines must both complete the same depth, at least 10. The second-best score must be below 80%, unless the best line forces mate and the second-best does not. |
| Great | The engine's best move **and** the second-best move is ≥ 15 percentage points worse **and** the mover's expected points afterwards are ≥ 35% **and** the second-best score is below 90%. Both lines must complete the same depth, at least 10. Excluded: plain recaptures, captures of hanging material (SEE ≥ 2), checkmates, queen promotions, and check evasions with ≤ 2 legal moves. |
| Miss | A base Mistake/Blunder immediately after an opponent loss of ≥ 10 points. The mover's best-move score must have risen ≥ 10 points over the previous best-position score, and the played move must retain that earlier score within 5 points. It "throws the gift back". |
| Best | The engine's first choice. |
| Excellent | loss ≤ 2 percentage points |
| Good | 2 < loss ≤ 5 |
| Inaccuracy | 5 < loss ≤ 10 |
| Mistake | 10 < loss ≤ 20 |
| Blunder | loss > 20 |

Boundary comparisons allow a tiny floating-point tolerance (10⁻¹² expected points). With only one MultiPV line,
Brilliant and Great lack the required alternative evidence and fall back to the base rating.

**Sacrifice:** after the move, the opponent can win at least 2 points of material (static exchange evaluation with
real legal moves, so pins and x-rays are respected) more than the move itself captured. Only the moved piece, or a
piece that becomes capturable *because of* the move, counts — pieces that were already en prise do not.
If, after the opponent takes, the mover can immediately win back at least as much (an in-between move or
desperado), it is not a sacrifice. Nor is it when the engine's best line takes the offered piece and the mover is
back within 1 point four plies later, finishing any capture sequence in progress (for example the fork trick 4. Nxe5
Nxe5 5. d4). This check is skipped when the best line forces mate.

## 4. Accuracy and centipawn loss

- **Move accuracy** (Lichess): `103.1668 · exp(−0.04354 · drop) − 3.1669 + 1`, clamped to 0–100, where `drop` is
  `100 · loss` from the comparable root scores above. Zero loss gives 100.
- **Game accuracy** (Lichess): the average of a volatility-weighted mean (weights = standard deviation of winning
  chances in a sliding window, clamped 0.5–12) and the harmonic mean of the move accuracies. Sharp positions weigh more,
  and one blunder isn't hidden by many easy moves.
- **ACPL:** mean non-negative centipawn loss of non-book moves, using the same best/played root scores from the mover's
  perspective, with evaluations capped at ±1000.

## 5. Explanations and tactical tags

Computed for inaccuracies, mistakes, misses, blunders and detected missed mates:

| Tag | Rule |
|---|---|
| allowed_mate | After the move the opponent has a forced mate (and didn't before). `back_rank` if the final mate is delivered by a rook/queen along the king's back rank. |
| missed_mate | A mate in ≤ 3 was available and the move doesn't keep a mate. |
| hanging_piece | The opponent's best reply captures one of the mover's pieces and the line loses ≥ 2 points of material. |
| allows_fork | The opponent's best reply attacks two or more valuable targets (king, higher-value or undefended pieces) and the line loses ≥ 2 points. |
| loses_material | Material balance after the opponent's best line (up to 6 plies, extended by at most 2 capturing plies) is ≥ 2 points lower. |
| missed_material / missed_fork | The best line would have won ≥ 2 points of material and retained ≥ 2 more than the played line (with a fork as the first move). The best line is inspected for 7 plies, extended by at most 2 capturing plies. |
| threat_ignored | Null-move test: had the mover passed, the opponent would gain ≥ 15 points — and the opponent's refutation of the played move executes that same threat. |
| self_inflicted | A mistake/blunder with no detected ignored threat or missed punish. |
| tactical / positional | Tactical if any concrete material, mate or ignored-threat tag applies, otherwise positional. |
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
  that compares with your opponents (×0.6–1.6), and rank areas with sufficient data. Areas with insufficient samples
  remain visible with no rank (`rank: null`). Areas overlap — one blunder can both hang a piece and ignore a threat.
- **Recurring mistakes:** identical positions (same placement, side to move, castling and en passant) reached in two or
  more different games where you lost ≥ 5 points.
- **Tilt:** score in games started within 30 minutes of a loss vs. your overall score; flagged only when the Wilson
  interval lies entirely below the baseline.
- **Performance rating:** average opponent rating + 400 × (wins − losses) / games (linear approximation).

## 7. Known limitations

- Moderate-depth engine analysis can misjudge very deep tactics; re-analyse a game deeper from the review page.
- The time cap may return a shallower completed iteration. Paired candidate searches and threat tests add engine work;
  the native implementation does not imply that a new game's engine analysis is faster than the previous pipeline.
- Brilliant/Great are heuristics; they will not always match human judgement or Chess.com's labels.
- Expected points are a transparent logistic heuristic. Material, fork and threat tags inspect bounded lines and cannot
  prove every continuation; absence of a tag is not proof that no tactic exists.
- Premoves, mouse slips and lag are invisible to us. Time statistics require clock data (live games include it).
- Chess.com's public API can lag a few minutes behind the website for brand-new games.
