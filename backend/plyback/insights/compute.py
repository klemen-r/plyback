"""Account-wide insights computed from stored games and per-move analysis.

Result-based statistics (results, ratings, openings, sessions) use every game in scope.
Engine-based statistics (errors, phases, time, conversion, priorities) use analysed
games only, and every section reports its sample size and a confidence label so
weak signals are never presented as strong patterns.  Where possible the player is
compared with their opponents in the same games — a like-for-like baseline because
Chess.com pairs players of similar strength.
"""

from __future__ import annotations

import math
import sys
import threading
import weakref
from collections import Counter, OrderedDict, defaultdict, namedtuple
from dataclasses import dataclass, field
from datetime import datetime, timezone

from ..analysis.pgn import parse_time_control
from ..db import Database, loads
from .resources import RESOURCES, lichess_analysis_url, opening_resources

ERRORS = ("mistake", "blunder", "miss")
_NO_TAGS: dict = {}
RECURRING_MIN_LOSS = 0.05
CLASSES = ("brilliant", "great", "best", "excellent", "good", "book", "forced", "inaccuracy", "mistake", "miss", "blunder")
DRAWS = {"agreed", "repetition", "stalemate", "insufficient", "50move", "timevsinsufficient"}
LOSS_REASONS = {
    "checkmated": "checkmate",
    "resigned": "resignation",
    "timeout": "time",
    "abandoned": "abandonment",
    "lose": "other",
    "kingofthehill": "other",
    "threecheck": "other",
    "bughousepartnerlose": "other",
}


@dataclass
class Filters:
    since: int | None = None
    until: int | None = None
    time_classes: list[str] | None = None
    color: str | None = None
    rated: bool | None = None
    opening: str | None = None
    opponent: str | None = None

    @staticmethod
    def from_dict(d: dict | None) -> Filters:
        if d is None:
            d = {}
        if not isinstance(d, dict):
            raise ValueError("filters must be an object")

        def timestamp(key: str) -> int | None:
            raw = d.get(key)
            if raw is None or raw == "":
                return None
            if isinstance(raw, bool) or not isinstance(raw, (int, str)):
                raise ValueError(f"{key} must be an integer Unix timestamp")
            if isinstance(raw, str) and (len(raw) > 16 or not raw.strip().isdigit()):
                raise ValueError(f"{key} must be an integer Unix timestamp")
            value = int(raw)
            if not 0 <= value <= 253_402_300_799:
                raise ValueError(f"{key} must be between 0 and 253402300799")
            return value

        def text(key: str, limit: int) -> str | None:
            raw = d.get(key)
            if raw is None:
                return None
            if not isinstance(raw, str) or len(raw) > limit:
                raise ValueError(f"{key} must be text of at most {limit} characters")
            return raw or None

        since, until = timestamp("since"), timestamp("until")
        if since is not None and until is not None and since > until:
            raise ValueError("since must not be later than until")
        tcs = d.get("time_classes")
        if isinstance(tcs, str):
            if len(tcs) > 64:
                raise ValueError("time_classes must contain at most four time classes")
            tcs = [t.strip() for t in tcs.split(",") if t.strip()]
        if tcs is not None:
            if not isinstance(tcs, list) or len(tcs) > 4:
                raise ValueError("time_classes must be a list of at most four time classes")
            if any(not isinstance(t, str) or t not in ("bullet", "blitz", "rapid", "daily") for t in tcs):
                raise ValueError("time_classes must contain bullet, blitz, rapid or daily")
            tcs = list(dict.fromkeys(tcs))
        color = text("color", 5)
        if color not in (None, "any", "white", "black"):
            raise ValueError("color must be white, black or any")
        rated = d.get("rated")
        if rated is None or rated == "" or rated == "any":
            rated = None
        elif isinstance(rated, bool):
            pass
        elif type(rated) is int and rated in (0, 1):
            rated = bool(rated)
        elif isinstance(rated, str) and rated.lower() in ("1", "0", "true", "false", "yes", "no"):
            rated = rated.lower() in ("1", "true", "yes")
        else:
            raise ValueError("rated must be true, false or any")
        return Filters(
            since=since,
            until=until,
            time_classes=tcs or None,
            color=color if color != "any" else None,
            rated=rated,
            opening=text("opening", 512),
            opponent=text("opponent", 256),
        )

    def to_dict(self) -> dict:
        return {k: v for k, v in self.__dict__.items() if v is not None}


def confidence(n: int, lo: int = 5, mid: int = 15, hi: int = 40) -> str:
    if n < lo:
        return "insufficient"
    if n < mid:
        return "low"
    if n < hi:
        return "moderate"
    return "high"


def wilson(k: float, n: int, z: float = 1.96) -> tuple[float, float]:
    if n == 0:
        return 0.0, 1.0
    p = k / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return max(0.0, centre - half), min(1.0, centre + half)


def _pct(a: float, b: float) -> float | None:
    return round(100 * a / b, 1) if b else None


def _avg(xs) -> float | None:
    xs = [x for x in xs if x is not None]
    return round(sum(xs) / len(xs), 2) if xs else None


@dataclass
class G:
    """One game from the account's perspective."""

    id: int
    row: dict
    color: str  # 'white' / 'black'
    score: float | None  # 1 / 0.5 / 0; None when stored without a result (PGN "*")
    outcome: str  # win / draw / loss / unknown
    reason: str  # how it ended
    user_rating: int | None
    opp_rating: int | None
    opponent: str
    end_time: int
    analyzed: bool
    accuracy: float | None = None
    opp_accuracy: float | None = None
    acpl: float | None = None
    chesscom_accuracy: float | None = None
    user_moves: list = field(default_factory=list)
    opp_moves: list = field(default_factory=list)

    @property
    def c(self) -> str:
        return "w" if self.color == "white" else "b"


MOVE_COLUMNS = (
    "game_id",
    "ply",
    "color",
    "san",
    "classification",
    "phase",
    "ep_loss",
    "cp_loss",
    "accuracy",
    "win_before",
    "win_after",
    "clock",
    "time_spent",
    "is_book",
    "tags",
    "epd_before",
    "best_san",
    "endgame_type",
)
Move = namedtuple("Move", MOVE_COLUMNS)


class MoveCache:
    """Per-game move rows kept in memory between insight computations.

    Entries are keyed by the game's analysis revision so a re-analysis invalidates them; only
    games whose analysis changed are read from the database again."""

    def __init__(self, max_moves: int = 750_000):
        self._data: OrderedDict[int, tuple[int, tuple[Move, ...]]] = OrderedDict()
        self._moves = 0
        self._max = max_moves
        self._lock = threading.Lock()

    def get_many(self, db: Database, versions: dict[int, int]) -> dict[int, tuple[Move, ...]]:
        with self._lock:
            missing = [gid for gid, v in versions.items() if gid not in self._data or self._data[gid][0] != v]
        loaded: dict[int, list[Move]] = {gid: [] for gid in missing}
        cols = ", ".join(MOVE_COLUMNS)
        conn = db.conn()
        for i in range(0, len(missing), 500):
            chunk = missing[i : i + 500]
            cur = conn.execute(
                f"SELECT {cols} FROM moves WHERE game_id IN ({','.join('?' * len(chunk))}) ORDER BY game_id, ply", chunk
            )
            cur.row_factory = None
            intern = sys.intern
            for gid, ply, color, san, cls, phase, ep, cpl, acc, wb, wa, clock, spent, book, tags, epd, best, egt in cur:
                ep = ep or 0.0
                loaded[gid].append(
                    Move(
                        gid,
                        ply,
                        intern(color),
                        san,
                        intern(cls) if cls else cls,
                        intern(phase) if phase else phase,
                        ep,
                        cpl,
                        acc,
                        wb,
                        wa,
                        clock,
                        spent,
                        book,
                        (loads(tags, None) or _NO_TAGS) if tags and tags != "{}" else _NO_TAGS,
                        epd if ep >= RECURRING_MIN_LOSS else None,
                        best if ep > 0 else None,
                        intern(egt) if egt else None,
                    )
                )
        out: dict[int, tuple[Move, ...]] = {}
        with self._lock:
            for gid, rows in loaded.items():
                old = self._data.pop(gid, None)
                self._moves -= len(old[1]) if old else 0
                self._data[gid] = (versions[gid], tuple(rows))
                self._moves += len(rows)
            for gid in versions:
                entry = self._data.get(gid)
                if entry is not None:
                    self._data.move_to_end(gid)
                    out[gid] = entry[1]
            while self._moves > self._max and self._data:
                _, (_, rows) = self._data.popitem(last=False)
                self._moves -= len(rows)
        return out


_MOVE_CACHES: weakref.WeakKeyDictionary[Database, MoveCache] = weakref.WeakKeyDictionary()
_MOVE_CACHES_LOCK = threading.Lock()


def _move_cache(db: Database) -> MoveCache:
    with _MOVE_CACHES_LOCK:
        cache = _MOVE_CACHES.get(db)
        if cache is None:
            cache = MoveCache()
            _MOVE_CACHES[db] = cache
        return cache


def _judge(ures: str | None, ores: str | None) -> tuple[float | None, str, str]:
    """Score, outcome and how it ended. A game without a result is left out of every score."""
    if not ures and not ores:
        return None, "unknown", ""
    if ures == "win":
        return 1.0, "win", LOSS_REASONS.get(ores, "other")
    if ures in DRAWS or ores in DRAWS:
        return 0.5, "draw", ures if ures in DRAWS else ores
    return 0.0, "loss", LOSS_REASONS.get(ures, "other")


def _decided(games: list[G]) -> list[G]:
    return [g for g in games if g.score is not None]


def _timeline(db: Database, account_id: int) -> list[tuple]:
    """Every game of the account in time order: (id, daily, start, end, outcome).

    Sessions are sittings at the board, so a filtered view still needs the games it hides."""
    rows = db.query(
        """SELECT g.id, g.time_class, g.start_time, g.end_time, ag.color, g.white_result, g.black_result
           FROM account_games ag JOIN games g ON g.id = ag.game_id
           WHERE ag.account_id = ? ORDER BY g.end_time, g.id""",
        [account_id],
    )
    out = []
    for r in rows:
        white = r["color"] == "white"
        ures, ores = (r["white_result"], r["black_result"]) if white else (r["black_result"], r["white_result"])
        end = r["end_time"] or 0
        out.append((r["id"], r["time_class"] == "daily", r["start_time"] or end, end, _judge(ures, ores)[1]))
    return out


def _load(db: Database, account_id: int, f: Filters) -> list[G]:
    where = ["ag.account_id = ?"]
    params: list = [account_id]
    if f.since is not None:
        where.append("g.end_time >= ?")
        params.append(f.since)
    if f.until is not None:
        where.append("g.end_time < ?")
        params.append(f.until)
    if f.time_classes:
        where.append(f"g.time_class IN ({','.join('?' for _ in f.time_classes)})")
        params += f.time_classes
    if f.color:
        where.append("ag.color = ?")
        params.append(f.color)
    if f.rated is not None:
        where.append("g.rated = ?")
        params.append(1 if f.rated else 0)
    if f.opening:
        where.append("g.opening_family = ?")
        params.append(f.opening)
    if f.opponent:
        where.append("(LOWER(g.white_username) = LOWER(?) OR LOWER(g.black_username) = LOWER(?))")
        params += [f.opponent, f.opponent]
    rows = db.query(
        f"""
        SELECT g.id, g.url, g.time_control, g.time_class, g.rated, g.start_time, g.end_time, g.white_username,
               g.white_rating, g.white_result, g.black_username, g.black_rating, g.black_result, g.result, g.plies,
               g.chesscom_accuracy_white, g.chesscom_accuracy_black, g.first_moves, g.opening_eco, g.opening_name,
               g.opening_family, g.opening_variation, g.rules, ag.color,
               ga.game_id AS analyzed, ga.revision, ga.accuracy_white, ga.accuracy_black, ga.acpl_white, ga.acpl_black
        FROM account_games ag JOIN games g ON g.id = ag.game_id
        LEFT JOIN game_analysis ga ON ga.game_id = g.id
        WHERE {" AND ".join(where)} ORDER BY g.end_time, g.id""",
        params,
    )
    games: list[G] = []
    for r in rows:
        white = r["color"] == "white"
        ures = r["white_result"] if white else r["black_result"]
        ores = r["black_result"] if white else r["white_result"]
        score, outcome, reason = _judge(ures, ores)
        g = G(
            id=r["id"],
            row=r,
            color=r["color"],
            score=score,
            outcome=outcome,
            reason=reason,
            user_rating=r["white_rating"] if white else r["black_rating"],
            opp_rating=r["black_rating"] if white else r["white_rating"],
            opponent=(r["black_username"] if white else r["white_username"]) or "?",
            end_time=r["end_time"] or 0,
            analyzed=r["analyzed"] is not None,
            accuracy=r["accuracy_white"] if white else r["accuracy_black"],
            opp_accuracy=r["accuracy_black"] if white else r["accuracy_white"],
            acpl=r["acpl_white"] if white else r["acpl_black"],
            chesscom_accuracy=r["chesscom_accuracy_white"] if white else r["chesscom_accuracy_black"],
        )
        games.append(g)
    versions = {g.id: g.row["revision"] for g in games if g.analyzed}
    moves_by_game = _move_cache(db).get_many(db, versions)
    for g in games:
        for m in moves_by_game.get(g.id, ()):
            (g.user_moves if m.color == g.c else g.opp_moves).append(m)
    return games


# --- helpers ------------------------------------------------------------------------------
def _wdl(gs: list[G]) -> dict:
    gs = _decided(gs)
    n = len(gs)
    w = sum(1 for g in gs if g.outcome == "win")
    d = sum(1 for g in gs if g.outcome == "draw")
    l_ = n - w - d
    pts = w + 0.5 * d
    lo, hi = wilson(pts, n)
    return {
        "games": n,
        "wins": w,
        "draws": d,
        "losses": l_,
        "score": _pct(pts, n),
        "score_ci": [round(100 * lo, 1), round(100 * hi, 1)],
        "confidence": confidence(n),
    }


def _example(g: G, m: dict) -> dict:
    """Evidence position. ``fen`` / ``explanation`` / ``lichess_url`` are filled in by ``_hydrate``
    so the bulk load never has to read those large columns."""
    return {
        "game_id": g.id,
        "ply": m.ply,
        "fen": None,
        "san": m.san,
        "best_san": m.best_san,
        "classification": m.classification,
        "ep_loss": m.ep_loss,
        "explanation": None,
        "phase": m.phase,
        "opponent": g.opponent,
        "end_time": g.end_time,
        "url": g.row.get("url"),
        "color": g.color,
        "outcome": g.outcome,
        "time_class": g.row.get("time_class"),
        "lichess_url": None,
    }


def _hydrate(db: Database, result: dict) -> None:
    examples: list[dict] = []

    def walk(node) -> None:
        if isinstance(node, dict):
            if "lichess_url" in node and node.get("fen") is None and "ply" in node:
                examples.append(node)
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    walk(result)
    keys = list({(e["game_id"], e["ply"]) for e in examples})
    found: dict[tuple[int, int], tuple[str, str]] = {}
    for i in range(0, len(keys), 400):
        chunk = keys[i : i + 400]
        values = ",".join("(?,?)" for _ in chunk)
        for r in db.query(
            f"SELECT game_id, ply, fen_before, explanation FROM moves WHERE (game_id, ply) IN (VALUES {values})",
            [x for k in chunk for x in k],
        ):
            found[(r["game_id"], r["ply"])] = (r["fen_before"], r["explanation"])
    for e in examples:
        fen, expl = found.get((e["game_id"], e["ply"]), (None, None))
        e["fen"], e["explanation"] = fen, expl
        e["lichess_url"] = lichess_analysis_url(fen) if fen else None
    for r in result.get("recurring", []):
        r.update(fen=r["example"]["fen"], lichess_url=r["example"]["lichess_url"], explanation=r["example"]["explanation"])


def _top_examples(pairs: list[tuple[G, dict]], k: int = 4) -> list[dict]:
    pairs = sorted(pairs, key=lambda p: -(p[1].ep_loss or 0))
    out, seen = [], set()
    for g, m in pairs:
        if g.id in seen:
            continue
        seen.add(g.id)
        out.append(_example(g, m))
        if len(out) >= k:
            break
    return out


def _user_pov_win(g: G, m: dict) -> float:
    """User's win% after move m (m may be either side's)."""
    return m.win_after if m.color == g.c else 100 - m.win_after


# --- sections -----------------------------------------------------------------------------
def _overview(games: list[G], analyzed: list[G]) -> dict:
    user_moves = [m for g in analyzed for m in g.user_moves]
    opp_moves = [m for g in analyzed for m in g.opp_moves]

    user_counts = Counter(m.classification for m in user_moves)
    opp_counts = Counter(m.classification for m in opp_moves)

    def per_game(counts: Counter, cls: str) -> float | None:
        return round(counts[cls] / len(analyzed), 2) if analyzed else None

    by_tc = []
    for tc in sorted({g.row["time_class"] for g in games if g.row["time_class"]}):
        sub = [g for g in games if g.row["time_class"] == tc]
        by_tc.append({"time_class": tc, **_wdl(sub), "accuracy": _avg(g.accuracy for g in sub if g.analyzed)})
    perf = None
    rated = [g for g in games if g.opp_rating and g.score is not None]
    if rated:
        avg_opp = sum(g.opp_rating for g in rated) / len(rated)
        w = sum(1 for g in rated if g.outcome == "win")
        l_ = sum(1 for g in rated if g.outcome == "loss")
        perf = round(avg_opp + 400 * (w - l_) / len(rated))
    return {
        **_wdl(games),
        "white": _wdl([g for g in games if g.color == "white"]),
        "black": _wdl([g for g in games if g.color == "black"]),
        "by_time_class": by_tc,
        "analyzed": len(analyzed),
        "accuracy": _avg(g.accuracy for g in analyzed),
        "opp_accuracy": _avg(g.opp_accuracy for g in analyzed),
        "acpl": _avg(g.acpl for g in analyzed),
        "chesscom_accuracy": _avg(g.chesscom_accuracy for g in games),
        "chesscom_accuracy_games": sum(1 for g in games if g.chesscom_accuracy is not None),
        "performance_rating": perf,
        "avg_opponent_rating": round(sum(g.opp_rating for g in rated) / len(rated)) if rated else None,
        "per_game": {cls: per_game(user_counts, cls) for cls in CLASSES},
        "opp_per_game": {cls: per_game(opp_counts, cls) for cls in CLASSES},
        "first_game": games[0].end_time if games else None,
        "last_game": games[-1].end_time if games else None,
    }


def _pool(g: G) -> str:
    """The rating pool a game counts for: its time class, and separately for Chess960."""
    time_class = g.row["time_class"] or ""
    return f"{time_class} 960" if g.row["rules"] == "chess960" else time_class


def _ratings(games: list[G]) -> dict:
    series: dict[str, list] = defaultdict(list)
    for g in games:
        if g.user_rating and g.row["time_class"] and g.row["rated"]:
            series[_pool(g)].append({"t": g.end_time, "rating": g.user_rating, "game_id": g.id})
    out = {}
    for tc, pts in series.items():
        step = max(1, len(pts) // 400)
        sampled = pts[::step]
        if sampled[-1] is not pts[-1]:
            sampled.append(pts[-1])
        ratings = [p["rating"] for p in pts]
        out[tc] = {
            "points": sampled,
            "start": ratings[0],
            "end": ratings[-1],
            "change": ratings[-1] - ratings[0],
            "peak": max(ratings),
            "low": min(ratings),
            "games": len(pts),
        }
    return out


def _results(games: list[G]) -> dict:
    games = _decided(games)
    wins = Counter(g.reason for g in games if g.outcome == "win")
    losses = Counter(g.reason for g in games if g.outcome == "loss")
    draws = Counter(g.reason for g in games if g.outcome == "draw")
    buckets = [
        (-10_000, -150, "≤ −150"),
        (-150, -50, "−150 to −50"),
        (-50, 50, "±50"),
        (50, 150, "+50 to +150"),
        (150, 10_000, "≥ +150"),
    ]
    by_opp = []
    for lo, hi, label in buckets:
        sub = [g for g in games if g.user_rating and g.opp_rating and lo <= g.opp_rating - g.user_rating < hi]
        if not sub:
            continue
        expected = sum(1 / (1 + 10 ** ((g.opp_rating - g.user_rating) / 400)) for g in sub) / len(sub)
        by_opp.append({"label": label, **_wdl(sub), "expected": round(100 * expected, 1)})
    hours: list[list[G]] = [[] for _ in range(24)]
    for g in games:
        if g.end_time:
            hours[datetime.fromtimestamp(g.end_time, timezone.utc).hour].append(g)
    by_hour = [
        {"hour_utc": h, "games": len(sub), "score": _pct(sum(g.score for g in sub), len(sub))}
        for h, sub in enumerate(hours)
    ]
    length = []
    for lo, hi, label in [(0, 40, "< 20 moves"), (40, 80, "20–40"), (80, 120, "40–60"), (120, 10_000, "60+")]:
        sub = [g for g in games if g.row["plies"] is not None and lo <= g.row["plies"] < hi]
        if sub:
            length.append({"label": label, **_wdl(sub)})
    return {
        "wins_by": dict(wins),
        "losses_by": dict(losses),
        "draws_by": dict(draws),
        "by_opponent": by_opp,
        "by_hour_utc": by_hour,
        "by_length": length,
    }


def _sessions(games: list[G], timeline: list[tuple]) -> dict:
    """Behaviour across playing sessions (games < 30 min apart), incl. results after a loss.

    Sessions are found on the whole account timeline; only the shown games are reported."""
    shown = {g.id: g for g in games}
    after = {"win": [], "loss": [], "draw": []}
    position_in_session: dict[str, list[float]] = defaultdict(list)
    sessions = 0
    prev: tuple | None = None
    idx = 0
    counted = False
    for gid, daily, start, end, outcome in timeline:
        # A daily game lasts days: not part of a sitting, never the previous game of a live one.
        if daily:
            continue
        # A game that started before the previous one ended does not continue it either.
        new = prev is None or not end or not 0 <= start - prev[3] <= 1800
        if new:
            idx = 0
            counted = False
        idx += 1
        g = shown.get(gid)
        if g is not None:
            if not counted:
                sessions += 1
                counted = True
            if g.score is not None:
                if not new and prev[4] in after:
                    after[prev[4]].append(g.score)
                position_in_session["1–3" if idx <= 3 else "4–6" if idx <= 6 else "7+"].append(g.score)
        prev = (gid, daily, start, end, outcome)
    games = _decided(games)
    base = sum(g.score for g in games) / len(games) if games else 0
    res = {
        k: {"games": len(v), "score": _pct(sum(v), len(v)), "confidence": confidence(len(v), 10, 30, 80)}
        for k, v in after.items()
    }
    tilt = None
    # Shown from the same sample size at which the after-loss figures stop being "insufficient".
    if res["loss"]["confidence"] != "insufficient":
        diff = (sum(after["loss"]) / len(after["loss"])) - base
        lo, hi = wilson(sum(after["loss"]), len(after["loss"]))
        tilt = {
            "score_after_loss": res["loss"]["score"],
            "baseline": round(100 * base, 1),
            "difference": round(100 * diff, 1),
            "significant": hi < base,
            "confidence": res["loss"]["confidence"],
        }
    return {
        "sessions": sessions,
        "after": res,
        "by_game_in_session": {k: {"games": len(v), "score": _pct(sum(v), len(v))} for k, v in position_in_session.items()},
        "tilt": tilt,
    }


def _trend(games: list[G]) -> list[dict]:
    if not games:
        return []
    span = games[-1].end_time - games[0].end_time
    monthly = span > 150 * 86400
    buckets: dict[str, list[G]] = defaultdict(list)
    for g in games:
        d = datetime.fromtimestamp(g.end_time, timezone.utc)
        if monthly:
            key = d.strftime("%Y-%m")
        else:
            iso = d.isocalendar()
            key = f"{iso.year}-W{iso.week:02d}"
        buckets[key].append(g)
    out = []
    for key in sorted(buckets):
        sub = buckets[key]
        an = [g for g in sub if g.analyzed]
        scored = _decided(sub)
        # Ratings of different time classes, and of Chess960, are separate pools: a mixed period has
        # no single rating.
        one_pool = len({_pool(g) for g in sub}) == 1
        out.append(
            {
                "period": key,
                "games": len(sub),
                "score": _pct(sum(g.score for g in scored), len(scored)),
                "accuracy": _avg(g.accuracy for g in an),
                "analyzed": len(an),
                "blunders_per_game": round(
                    sum(1 for g in an for m in g.user_moves if m.classification == "blunder") / len(an), 2
                )
                if an
                else None,
                "rating": sub[-1].user_rating if one_pool else None,
            }
        )
    return out


def _opening_exit_win(g: G) -> float | None:
    moves = sorted(g.user_moves + g.opp_moves, key=lambda m: m.ply)
    if not moves:
        return None
    target = next((m for m in moves if m.ply == 20), moves[-1] if len(moves) < 20 else None)
    return _user_pov_win(g, target) if target else None


def _openings(games: list[G], overall_score: float) -> dict:
    out = {}
    for color in ("white", "black"):
        sub = [g for g in games if g.color == color]
        fams: dict[str, list[G]] = defaultdict(list)
        for g in sub:
            fams[g.row["opening_family"] or "Unknown"].append(g)
        fam_rows = []
        for fam, gs in fams.items():
            variations: dict[str, list[G]] = defaultdict(list)
            for g in gs:
                variations[g.row["opening_name"] or fam].append(g)
            an = [g for g in gs if g.analyzed]
            opening_moves = [m for g in an for m in g.user_moves if m.phase == "opening" and not m.is_book]
            exit_wins = [w for w in (_opening_exit_win(g) for g in an) if w is not None]
            wdl = _wdl(gs)
            decided = wdl["games"]
            flag = None
            if decided >= 5:
                score = wdl["score"] or 0
                if wdl["score_ci"][1] < overall_score or (score <= overall_score - 15 and decided >= 8):
                    flag = "weakness"
                elif wdl["score_ci"][0] > overall_score or (score >= overall_score + 15 and decided >= 8):
                    flag = "strength"
            fam_rows.append(
                {
                    "family": fam,
                    **wdl,
                    "accuracy": _avg(g.accuracy for g in an),
                    "opening_accuracy": _avg(m.accuracy for m in opening_moves),
                    "opening_errors_per_game": round(
                        sum(1 for m in opening_moves if m.classification in ERRORS) / len(an), 2
                    )
                    if an
                    else None,
                    "exit_win": round(sum(exit_wins) / len(exit_wins), 1) if exit_wins else None,
                    "analyzed": len(an),
                    "flag": flag,
                    "variations": sorted(
                        [{"name": v, **_wdl(vg)} for v, vg in variations.items()], key=lambda x: -x["games"]
                    )[:12],
                    "recent_game_ids": [g.id for g in gs[-5:]][::-1],
                    "resources": opening_resources(fam) if fam != "Unknown" else [],
                }
            )
        fam_rows.sort(key=lambda r: -r["games"])
        first: dict[str, list[G]] = defaultdict(list)
        for g in sub:
            fm = (g.row["first_moves"] or "").split()
            if color == "white" and fm:
                first[fm[0]].append(g)
            elif color == "black" and len(fm) >= 2:
                first[f"1.{fm[0]} {fm[1]}"].append(g)
        out[color] = {
            "families": fam_rows,
            "first_moves": sorted([{"move": k, **_wdl(v)} for k, v in first.items()], key=lambda x: -x["games"])[:12],
        }
    return out


def _rates(moves: list[dict]) -> dict:
    n = len(moves)
    c: Counter = Counter()
    accuracy_sum, accuracy_count, cp_sum = 0.0, 0, 0
    for m in moves:
        c[m.classification] += 1
        if m.accuracy is not None:
            accuracy_sum += m.accuracy
            accuracy_count += 1
        cp_sum += min(m.cp_loss or 0, 1000)
    return {
        "moves": n,
        "accuracy": round(accuracy_sum / accuracy_count, 2) if accuracy_count else None,
        "acpl": round(cp_sum / n, 2) if n else None,
        "inaccuracies_per_100": _pct(c["inaccuracy"], n),
        "mistakes_per_100": _pct(c["mistake"] + c["miss"], n),
        "blunders_per_100": _pct(c["blunder"], n),
    }


def _phases(analyzed: list[G]) -> dict:
    out = {}
    n_games = len(analyzed) or 1
    by_phase: dict[str, tuple[list, list]] = {ph: ([], []) for ph in ("opening", "middlegame", "endgame")}
    reaching: Counter = Counter()
    for g in analyzed:
        seen = set()
        for m in g.user_moves:
            seen.add(m.phase)
            if not m.is_book:
                by_phase[m.phase][0].append(m)
        for m in g.opp_moves:
            if not m.is_book:
                by_phase[m.phase][1].append(m)
        reaching.update(seen)
    for ph in ("opening", "middlegame", "endgame"):
        um, om = by_phase[ph]
        games_reaching = reaching[ph]
        out[ph] = {
            "user": _rates(um),
            "opponent": _rates(om),
            "games": games_reaching,
            "ep_lost_per_game": round(sum(m.ep_loss or 0 for m in um) / n_games, 3),
            "opp_ep_lost_per_game": round(sum(m.ep_loss or 0 for m in om) / n_games, 3),
            "confidence": confidence(len(um), 30, 150, 500),
        }
    # Endgame by type
    types: dict[str, dict] = defaultdict(lambda: {"user": [], "opp": [], "games": set()})
    for g in analyzed:
        for m in g.user_moves:
            if m.phase == "endgame":
                t = m.endgame_type or "mixed"
                types[t]["user"].append(m)
                types[t]["games"].add(g.id)
        for m in g.opp_moves:
            if m.phase == "endgame":
                types[m.endgame_type or "mixed"]["opp"].append(m)
    out["endgame_types"] = sorted(
        [
            {
                "type": t,
                "games": len(v["games"]),
                "user": _rates(v["user"]),
                "opponent": _rates(v["opp"]),
                "confidence": confidence(len(v["user"]), 20, 80, 250),
            }
            for t, v in types.items()
        ],
        key=lambda x: -x["games"],
    )
    return out


TAG_LABELS = {
    "hanging_piece": "Left a piece hanging",
    "threat_ignored": "Ignored an opponent's threat",
    "allowed_mate": "Allowed a forced mate",
    "back_rank": "Back-rank weakness",
    "allows_fork": "Allowed a fork",
    "loses_material": "Lost material to a tactic",
    "missed_punish": "Didn't punish the opponent's error",
    "missed_material": "Missed winning material",
    "missed_fork": "Missed a fork",
    "missed_mate": "Missed a forced mate",
    "self_inflicted": "Self-inflicted (no threat existed)",
    "positional": "Positional / strategic error",
    "tactical": "Tactical error",
    "in_check": "Wrong reply to a check",
    "time_pressure": "Error in time trouble",
}


def _errors(analyzed: list[G]) -> dict:
    n_games = len(analyzed) or 1
    tags_u: dict[str, list] = defaultdict(list)
    tags_o: dict[str, list] = defaultdict(list)
    hung = Counter()
    for g in analyzed:
        for m in g.user_moves:
            if m.classification in ERRORS:
                for t in m.tags:
                    tags_u[t].append((g, m))
                if "hanging_piece" in m.tags:
                    hung[m.tags["hanging_piece"].get("piece", "?")] += 1
            elif m.classification in ("good", "excellent", "inaccuracy") and "missed_mate" in m.tags:
                tags_u["missed_mate"].append((g, m))
        # The same predicate for both sides, so the opponents' figure is comparable.
        for m in g.opp_moves:
            if m.classification in ERRORS:
                for t in m.tags:
                    tags_o[t].append((g, m))
            elif m.classification in ("good", "excellent", "inaccuracy") and "missed_mate" in m.tags:
                tags_o["missed_mate"].append((g, m))
    rows = []
    for t, label in TAG_LABELS.items():
        u = tags_u.get(t, [])
        o = tags_o.get(t, [])
        rows.append(
            {
                "tag": t,
                "label": label,
                "count": len(u),
                "per_game": round(len(u) / n_games, 2),
                "opp_per_game": round(len(o) / n_games, 2),
                "ep_lost_per_game": round(sum(m.ep_loss or 0 for _, m in u) / n_games, 3),
                "examples": _top_examples(u, 3),
                "confidence": confidence(len(u)),
            }
        )
    rows.sort(key=lambda r: -r["ep_lost_per_game"])
    total_err = sum(1 for g in analyzed for m in g.user_moves if m.classification in ERRORS)
    tactical = len(tags_u.get("tactical", []))
    return {
        "tags": rows,
        "hung_pieces": dict(hung),
        "total_errors": total_err,
        "tactical_share": _pct(tactical, total_err),
        "positional_share": _pct(len(tags_u.get("positional", [])), total_err),
        "errors_per_game": round(total_err / n_games, 2),
        "opp_errors_per_game": round(
            sum(1 for g in analyzed for m in g.opp_moves if m.classification in ERRORS) / n_games, 2
        ),
    }


def _time(analyzed: list[G], games: list[G]) -> dict:
    timed = []
    for g in analyzed:
        tc = parse_time_control(g.row["time_control"])
        if tc.base and not tc.daily and any(m.clock is not None for m in g.user_moves):
            timed.append((g, tc))
    out: dict = {"games": len(timed), "confidence": confidence(len(timed), 5, 20, 60)}
    if not timed:
        return out
    times_u: dict[str, list[float]] = defaultdict(list)
    times_o: dict[str, list[float]] = defaultdict(list)
    curve_u: dict[int, list[float]] = defaultdict(list)
    curve_o: dict[int, list[float]] = defaultdict(list)
    trouble_games, low_moves, normal_moves = [], [], []
    speed_moves: list[list[Move]] = [[], [], [], []]
    for g, tc in timed:
        low = False
        threshold = max(10.0, 0.1 * tc.base)
        for m in g.user_moves:
            if m.time_spent is not None:
                times_u[m.phase].append(m.time_spent)
            if m.clock is not None:
                curve_u[(m.ply + 1) // 2].append(m.clock / tc.base)
                if not m.is_book:
                    if m.clock <= threshold:
                        low = True
                        low_moves.append(m)
                    else:
                        normal_moves.append(m)
                        if m.time_spent is not None:
                            fraction = m.time_spent / tc.base
                            if 0 <= fraction < 10:
                                bucket = 0 if fraction < 0.01 else 1 if fraction < 0.03 else 2 if fraction < 0.08 else 3
                                speed_moves[bucket].append(m)
        for m in g.opp_moves:
            if m.time_spent is not None:
                times_o[m.phase].append(m.time_spent)
            if m.clock is not None:
                curve_o[(m.ply + 1) // 2].append(m.clock / tc.base)
        if low:
            trouble_games.append(g)
    out["avg_time_per_move"] = {
        ph: {"user": _avg(times_u[ph]), "opponent": _avg(times_o[ph])}
        for ph in ("opening", "middlegame", "endgame")
    }
    out["clock_curve"] = [
        {
            "move": k,
            "user": round(100 * sum(curve_u[k]) / len(curve_u[k]), 1),
            "opponent": round(100 * sum(curve_o[k]) / len(curve_o[k]), 1) if curve_o.get(k) else None,
            "games": len(curve_u[k]),
        }
        for k in sorted(curve_u)
        if len(curve_u[k]) >= max(3, len(timed) // 10)
    ][:80]
    out["time_trouble"] = {
        "games": len(trouble_games),
        "share": _pct(len(trouble_games), len(timed)),
        "score": _wdl(trouble_games)["score"] if trouble_games else None,
        "errors_per_100_low": _pct(sum(1 for m in low_moves if m.classification in ERRORS), len(low_moves)),
        "errors_per_100_normal": _pct(sum(1 for m in normal_moves if m.classification in ERRORS), len(normal_moves)),
        "moves_low": len(low_moves),
    }
    # speed vs quality
    buckets = [(0, 0.01, "very fast"), (0.01, 0.03, "fast"), (0.03, 0.08, "normal"), (0.08, 10, "long think")]
    speed = []
    for (lo, hi, label), ms in zip(buckets, speed_moves, strict=True):
        speed.append(
            {
                "label": label,
                "moves": len(ms),
                "range": f"{lo * 100:g}–{hi * 100:g}% of base" if hi < 10 else f"> {lo * 100:g}% of base",
                "errors_per_100": _pct(sum(1 for m in ms if m.classification in ERRORS), len(ms)),
                "blunders_per_100": _pct(sum(1 for m in ms if m.classification == "blunder"), len(ms)),
                "accuracy": _avg(m.accuracy for m in ms),
            }
        )
    out["speed_vs_quality"] = speed
    # timeouts
    timeouts = [g for g in games if g.outcome == "loss" and g.reason == "time"]
    to_analyzed = [g for g in timeouts if g.analyzed and (g.user_moves or g.opp_moves)]
    not_losing = []
    for g in to_analyzed:
        last = max(g.user_moves + g.opp_moves, key=lambda m: m.ply)
        if _user_pov_win(g, last) >= 50:
            not_losing.append(g)
    out["timeouts"] = {
        "losses_on_time": len(timeouts),
        "share_of_losses": _pct(len(timeouts), sum(1 for g in games if g.outcome == "loss")),
        "in_non_losing_position": len(not_losing),
        "analyzed": len(to_analyzed),
        "wins_on_time": sum(1 for g in games if g.outcome == "win" and g.reason == "time"),
        "examples": [{"game_id": g.id, "opponent": g.opponent, "end_time": g.end_time} for g in not_losing[-5:]],
    }
    end_clock = {"win": [], "loss": [], "draw": []}
    for g, tc in timed:
        cl = [m.clock for m in g.user_moves if m.clock is not None]
        if cl and g.outcome in end_clock:
            end_clock[g.outcome].append(100 * cl[-1] / tc.base)
    out["clock_left_at_end"] = {k: _avg(v) for k, v in end_clock.items()}
    return out


def _advantage(analyzed: list[G]) -> dict:
    had_win, converted, lost_adv = [], [], []
    had_lost, saved = [], []
    decisive = Counter()
    decisive_examples: dict[str, list] = defaultdict(list)
    # Without a result a game was neither converted, saved nor lost.
    for g in _decided(analyzed):
        moves = sorted(g.user_moves + g.opp_moves, key=lambda m: m.ply)
        series = [(m, _user_pov_win(g, m)) for m in moves if m.ply >= 10]
        if not series:
            continue
        peak_m, peak = max(series, key=lambda x: x[1])
        low = min(w for _, w in series)
        if peak >= 80:
            had_win.append(g)
            if g.outcome == "win":
                converted.append(g)
            else:
                after_peak = [m for m in g.user_moves if m.ply > peak_m.ply]
                culprit = max(after_peak, key=lambda m: m.ep_loss or 0, default=None)
                lost_adv.append((g, peak, culprit))
        if low <= 20:
            had_lost.append(g)
            if g.outcome != "loss":
                saved.append(g)
        if g.outcome == "loss" and g.user_moves:
            worst = max(g.user_moves, key=lambda m: m.ep_loss or 0)
            if (worst.ep_loss or 0) >= 0.2:
                t = worst.tags
                kind = next(
                    (
                        k
                        for k in (
                            "allowed_mate",
                            "hanging_piece",
                            "allows_fork",
                            "threat_ignored",
                            "missed_punish",
                            "loses_material",
                            "positional",
                        )
                        if k in t
                    ),
                    "other",
                )
                key = f"{worst.phase}:{kind}"
                decisive[key] += 1
                decisive_examples[key].append((g, worst))
            elif g.reason == "time":
                decisive["time:timeout"] += 1
            else:
                decisive["gradual:no single decisive error"] += 1
    lost_rows = []
    for g, peak, culprit in sorted(lost_adv, key=lambda x: -x[0].end_time)[:15]:
        lost_rows.append(
            {
                "game_id": g.id,
                "opponent": g.opponent,
                "end_time": g.end_time,
                "peak_win": round(peak, 1),
                "outcome": g.outcome,
                "reason": g.reason,
                "culprit": _example(g, culprit) if culprit else None,
            }
        )
    n_loss = sum(1 for g in analyzed if g.outcome == "loss")
    return {
        "winning_positions": len(had_win),
        "converted": len(converted),
        "conversion_rate": _pct(len(converted), len(had_win)),
        "conversion_confidence": confidence(len(had_win)),
        "losing_positions": len(had_lost),
        "saved": len(saved),
        "save_rate": _pct(len(saved), len(had_lost)),
        "lost_advantages": lost_rows,
        "how_you_lose": sorted(
            [
                {
                    "key": k,
                    "phase": k.split(":")[0],
                    "kind": k.split(":")[1],
                    "count": v,
                    "share": _pct(v, n_loss),
                    "examples": _top_examples(decisive_examples.get(k, []), 2),
                }
                for k, v in decisive.items()
            ],
            key=lambda x: -x["count"],
        ),
        "losses_analyzed": n_loss,
    }


def _missed(analyzed: list[G]) -> dict:
    misses, opp_errors_punished, opp_errors = [], 0, 0
    missed_mates = []
    for g in analyzed:
        moves = sorted(g.user_moves + g.opp_moves, key=lambda m: m.ply)
        for i, m in enumerate(moves):
            if m.color != g.c and m.classification in ("mistake", "blunder") and i + 1 < len(moves):
                opp_errors += 1
                reply = moves[i + 1]
                if (reply.ep_loss or 0) < 0.05:
                    opp_errors_punished += 1
                else:
                    misses.append((g, reply))
            if m.color == g.c and "missed_mate" in m.tags:
                missed_mates.append((g, m))
    return {
        "opponent_errors": opp_errors,
        "punished": opp_errors_punished,
        "punish_rate": _pct(opp_errors_punished, opp_errors),
        "confidence": confidence(opp_errors, 10, 30, 100),
        "missed_count": len(misses),
        "missed_mates": len(missed_mates),
        "examples": _top_examples(misses, 6),
        "missed_mate_examples": _top_examples(missed_mates, 4),
    }


def _recurring(analyzed: list[G]) -> list[dict]:
    groups: dict[str, list] = defaultdict(list)
    for g in analyzed:
        for m in g.user_moves:
            if m.ep_loss >= RECURRING_MIN_LOSS and not m.is_book and m.epd_before:
                groups[m.epd_before].append((g, m))
    out = []
    for items in groups.values():
        game_ids = {g.id for g, _ in items}
        if len(game_ids) < 2:
            continue
        played = Counter(m.san for _, m in items)
        best = Counter(m.best_san for _, m in items).most_common(1)[0][0]
        avg_loss = sum(m.ep_loss for _, m in items) / len(items)
        g0, m0 = max(items, key=lambda x: x[1].ep_loss)
        out.append(
            {
                "fen": None,
                "occurrences": len(items),
                "games": sorted(game_ids)[-8:],
                "played": [{"san": s, "count": c} for s, c in played.most_common()],
                "best_san": best,
                "avg_ep_loss": round(avg_loss, 3),
                "phase": m0.phase,
                "move_number": (m0.ply + 1) // 2,
                "color": g0.color,
                "explanation": None,
                "example": _example(g0, m0),
                "lichess_url": None,
                "impact": round(avg_loss * len(items), 3),
            }
        )
    out.sort(key=lambda r: -r["impact"])
    return out[:15]


AREAS = [
    # key, title, predicate(move) for errors, resource key
    ("hanging_piece", "Piece safety (hanging pieces)", lambda m: "hanging_piece" in m.tags, "hanging_piece"),
    ("threat_ignored", "Spotting your opponent's threats", lambda m: "threat_ignored" in m.tags, "threat_ignored"),
    ("allowed_mate", "King safety (allowing mates)", lambda m: "allowed_mate" in m.tags, "allowed_mate"),
    ("allows_fork", "Avoiding forks and double attacks", lambda m: "allows_fork" in m.tags, "allows_fork"),
    (
        "missed_tactics",
        "Punishing mistakes & finding tactics",
        lambda m: m.classification == "miss" or bool(m.tags.keys() & {"missed_material", "missed_fork", "missed_punish"}),
        "missed_tactics",
    ),
    ("conversion", "Converting winning positions", lambda m: (m.win_before or 0) >= 80, "conversion"),
    ("endgame", "Endgame technique", lambda m: m.phase == "endgame", "endgame"),
    ("opening", "Opening play", lambda m: m.phase == "opening", "opening"),
    ("time_pressure", "Time management", lambda m: "time_pressure" in m.tags, "time"),
    ("positional", "Positional decisions", lambda m: "positional" in m.tags and m.phase != "endgame", "positional"),
]


def _priorities(analyzed: list[G], openings: dict, time_info: dict, sessions: dict) -> list[dict]:
    n = len(analyzed)
    if n == 0:
        return []
    out = []
    flagged = set(ERRORS) | {"inaccuracy"}
    user_err = [(g, m) for g in analyzed for m in g.user_moves if m.classification in flagged]
    opp_err = [(g, m) for g in analyzed for m in g.opp_moves if m.classification in flagged]
    for key, title, pred, res in AREAS:
        u = [(g, m) for g, m in user_err if pred(m)]
        o = [(g, m) for g, m in opp_err if pred(m)]
        serious = [(g, m) for g, m in u if m.classification in ERRORS]
        ep_u = sum(m.ep_loss or 0 for _, m in u) / n
        ep_o = sum(m.ep_loss or 0 for _, m in o) / n
        ratio = (ep_u / ep_o) if ep_o > 0 else (2.0 if ep_u > 0 else 1.0)
        score = ep_u * max(0.6, min(1.6, ratio))
        conf = confidence(len(serious), 5, 15, 40)
        rate_u = len(serious) / n
        rate_o = sum(1 for _, m in o if m.classification in ERRORS) / n
        summary = _area_summary(key, rate_u, rate_o, ep_u, ratio, n)
        item = {
            "key": key,
            "title": title,
            "score": round(score, 4),
            "ep_lost_per_game": round(ep_u, 3),
            "opp_ep_lost_per_game": round(ep_o, 3),
            "vs_opponents": round(ratio, 2),
            "errors": len(serious),
            "errors_per_game": round(rate_u, 2),
            "opp_errors_per_game": round(rate_o, 2),
            "confidence": conf,
            "summary": summary,
            "examples": _top_examples(serious or u, 4),
            "resources": RESOURCES.get(res, []),
        }
        if key == "opening":
            weak = [
                f for c in ("white", "black") for f in openings.get(c, {}).get("families", []) if f["flag"] == "weakness"
            ]
            if weak:
                worst = min(weak, key=lambda f: f["score"] or 0)
                item["detail"] = f"Weakest opening: {worst['family']} — {worst['score']}% score over {worst['games']} games."
                item["resources"] = worst["resources"] + item["resources"]
        if key == "time_pressure" and time_info.get("timeouts", {}).get("in_non_losing_position"):
            t = time_info["timeouts"]
            item["detail"] = f"{t['in_non_losing_position']} games lost on time in positions that were not losing."
        out.append(item)
    ranked = sorted([x for x in out if x["confidence"] != "insufficient" and x["score"] > 0], key=lambda x: -x["score"])
    rest = [x for x in out if x not in ranked]
    for i, x in enumerate(ranked):
        x["rank"] = i + 1
    tilt = sessions.get("tilt")
    if tilt and tilt["significant"]:
        ranked.append(
            {
                "key": "tilt",
                "title": "Playing on after losses (tilt)",
                "rank": len(ranked) + 1,
                "confidence": tilt["confidence"],
                "score": 0,
                "summary": f"You score {tilt['score_after_loss']}% in games started right after a loss, versus "
                f"{tilt['baseline']}% overall. Taking a short break after a loss may help.",
                "examples": [],
                "resources": RESOURCES["tilt"],
            }
        )
    return ranked + [dict(x, rank=None) for x in rest]


def _area_summary(key: str, rate_u: float, rate_o: float, ep_u: float, ratio: float, n: int) -> str:
    cmp = ""
    if rate_o > 0:
        if ratio >= 1.25:
            cmp = f" — {ratio:.1f}× more costly than for your opponents"
        elif ratio <= 0.8:
            cmp = " — better than your opponents, but still a source of lost points"
        else:
            cmp = " — similar to your opponents"
    what = {
        "hanging_piece": "You left material en prise",
        "threat_ignored": "You missed an existing threat",
        "allowed_mate": "You allowed a forced mate",
        "allows_fork": "You allowed a fork",
        "missed_tactics": "You missed a tactic or failed to punish an error",
        "conversion": "You made a serious error while clearly winning",
        "endgame": "You made a serious endgame error",
        "opening": "You made a serious opening error",
        "time_pressure": "You made a serious error in time trouble",
        "positional": "You made a serious positional (non-tactical) error",
    }[key]
    return (
        f"{what} {rate_u:.2f} times per game (opponents: {rate_o:.2f}){cmp}. "
        f"This costs about {100 * ep_u:.0f} percentage points of winning chances per game."
    )


def compute_insights(db: Database, account_id: int, filters: Filters | dict | None = None) -> dict:
    f = filters if isinstance(filters, Filters) else Filters.from_dict(filters)
    games = _load(db, account_id, f)
    analyzed = [g for g in games if g.analyzed and g.user_moves]
    overview = _overview(games, analyzed)
    overall_score = overview["score"] or 0
    openings = _openings(games, overall_score)
    sessions = _sessions(games, _timeline(db, account_id))
    time_info = _time(analyzed, games)
    unanalyzed = [g.id for g in games if not g.analyzed and g.row["rules"] in ("chess", "chess960")]
    notes = []
    if len(analyzed) < 10:
        notes.append("Fewer than 10 analysed games: engine-based patterns are not yet reliable.")
    elif len(analyzed) < 40:
        notes.append("Fewer than 40 analysed games: treat engine-based patterns as preliminary.")
    if games and len(analyzed) < len(games):
        notes.append(
            f"{len(games) - len(analyzed)} game(s) in this selection are not analysed yet; "
            "result statistics include them, engine statistics do not."
        )
    unknown = len(games) - len(_decided(games))
    if unknown:
        notes.append(f"{unknown} game(s) in this selection have no recorded result; scores leave them out.")
    result = {
        "filters": f.to_dict(),
        "data_quality": {
            "games": len(games),
            "analyzed": len(analyzed),
            "coverage": _pct(len(analyzed), len(games)),
            "unanalyzed_ids": unanalyzed[-2000:],
            "notes": notes,
            "confidence": confidence(len(analyzed), 10, 40, 150),
        },
        "overview": overview,
        "ratings": _ratings(games),
        "results": _results(games),
        "sessions": sessions,
        "trend": _trend(games),
        "openings": openings,
        "phases": _phases(analyzed),
        "errors": _errors(analyzed),
        "time": time_info,
        "advantage": _advantage(analyzed),
        "missed": _missed(analyzed),
        "recurring": _recurring(analyzed),
        "priorities": _priorities(analyzed, openings, time_info, sessions),
    }
    _hydrate(db, result)
    return result
