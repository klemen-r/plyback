"""Natural-language explanations for classified moves.

Explanations are generated from concrete, verifiable facts (engine lines, material
counts along those lines, mate distances, static exchange evaluation) rather than
from free-form guesses.
"""

from __future__ import annotations

from .winprob import format_eval

LEADS = {
    "brilliant": "Brilliant!",
    "great": "Great move!",
    "best": "Best move.",
    "excellent": "Excellent move.",
    "good": "Good move.",
    "book": "Book move.",
    "forced": "Forced — the only legal move.",
    "inaccuracy": "Inaccuracy.",
    "mistake": "Mistake.",
    "miss": "Missed opportunity.",
    "blunder": "Blunder.",
}


def _join_line(sans: list[str], n: int = 4) -> str:
    return " ".join(sans[:n])


def explain(ctx: dict) -> str:
    """Build the explanation string from the analysis context of one move.

    Expected keys: cls, san, best_san, best_line (SAN list), reply_line (SAN list),
    eval_best, eval_after (White-POV cp), tags (dict tag -> detail), opening, second_san,
    eval_second, white (bool).
    """
    cls = ctx["cls"]
    tags: dict = ctx.get("tags", {})
    best_san = ctx.get("best_san")
    parts = [LEADS.get(cls, "")]

    if cls == "book":
        if ctx.get("opening"):
            parts.append(f"Opening theory ({ctx['opening']}).")
        return " ".join(p for p in parts if p)
    if cls == "forced":
        return parts[0]

    if cls == "brilliant":
        sac = tags.get("sacrifice", {}).get("piece", "material")
        parts.append(
            f"A sound sacrifice of the {sac}: the engine confirms it is the strongest continuation"
            f" ({format_eval(ctx['eval_after'])})."
        )
        if ctx.get("best_line"):
            parts.append(f"Main line: {_join_line(ctx['best_line'], 6)}.")
        return " ".join(parts)

    if cls == "great":
        second = ctx.get("second_san")
        keeps = "the advantage" if ctx.get("win_after", 50) >= 60 else "the balance"
        parts.append(f"The only move that keeps {keeps}.")
        if second and ctx.get("eval_second") is not None:
            parts.append(f"The next best option, {second}, would leave the evaluation at {format_eval(ctx['eval_second'])}.")
        return " ".join(parts)

    if cls in ("best", "excellent", "good"):
        if "missed_mate" in tags:
            m = tags["missed_mate"]
            parts.append(f"Still winning, but {best_san} would have forced mate in {m['n']}.")
        elif cls != "best" and best_san:
            parts.append(
                f"The engine slightly prefers {best_san} "
                f"({format_eval(ctx['eval_best'])} vs {format_eval(ctx['eval_after'])})."
            )
        return " ".join(parts)

    # Errors: inaccuracy / mistake / miss / blunder
    reasons: list[str] = []
    if "allowed_mate" in tags:
        t = tags["allowed_mate"]
        what = "a back-rank mate" if "back_rank" in tags else f"a forced mate in {t['n']}"
        reasons.append(f"This allows {what}: {_join_line(ctx.get('reply_line', []), min(2 * t['n'] - 1, 7))}.")
    if "missed_mate" in tags:
        m = tags["missed_mate"]
        reasons.append(f"You had a forced mate in {m['n']} starting with {best_san}.")
    if "hanging_piece" in tags:
        h = tags["hanging_piece"]
        reasons.append(f"It leaves your {h['piece']} on {h['square']} unprotected — {h['capture']} wins it.")
    elif "allows_fork" in tags:
        f = tags["allows_fork"]
        reasons.append(f"It allows {f['move']}, a fork of your {' and '.join(f['targets'])}.")
    elif "loses_material" in tags and "allowed_mate" not in tags:
        lm = tags["loses_material"]
        reasons.append(f"After {_join_line(ctx.get('reply_line', []), lm.get('plies', 4))} you lose {lm['words']}.")
    if "threat_ignored" in tags and "allowed_mate" not in tags:
        reasons.append(
            f"Your opponent was threatening {tags['threat_ignored']['move']}, and this move does not deal with it."
        )
    if cls == "miss" or "missed_punish" in tags:
        reasons.append("Your opponent's previous move was an error, but this move does not take advantage of it.")
    if "missed_fork" in tags and "missed_mate" not in tags:
        f = tags["missed_fork"]
        reasons.append(f"{f['move']} would have forked the {' and '.join(f['targets'])}.")
    elif "missed_material" in tags and "missed_mate" not in tags:
        mm = tags["missed_material"]
        reasons.append(f"{best_san} would have won {mm['words']}.")

    if not reasons:
        a, b = format_eval(ctx["eval_best"]), format_eval(ctx["eval_after"])
        wb, wa = ctx.get("win_before", 50), ctx.get("win_after", 50)
        if wb >= 60 and wa < wb:
            reasons.append(f"It lets part of your advantage slip (evaluation {a} → {b}).")
        elif wa <= 40:
            reasons.append(f"It makes your position harder to hold (evaluation {a} → {b}).")
        else:
            reasons.append(f"It hands your opponent the initiative (evaluation {a} → {b}).")
    parts.extend(reasons)
    if best_san and not any(best_san in r for r in reasons):
        line = ctx.get("best_line") or [best_san]
        parts.append(f"Better was {best_san} ({format_eval(ctx['eval_best'])}), e.g. {_join_line(line, 4)}.")
    return " ".join(p for p in parts if p)
