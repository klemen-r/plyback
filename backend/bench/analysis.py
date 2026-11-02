"""Measure real engine search separately from analysis post-processing.

    python bench/analysis.py --out analysis.json --record evaluations.json
    python bench/analysis.py --replay evaluations.json --profile analysis.prof

Defaults match the standard analysis budget. Files passed to --record contain PGNs
and engine lines; the benchmark never opens the application's database.
"""

from __future__ import annotations

import argparse
import cProfile
import json
import platform
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed  # noqa: E402
from plyback.analysis.engine import Engine, Line  # noqa: E402
from plyback.analysis.pgn import parse_pgn, split_games  # noqa: E402


def request_key(board, depth: int, multipv: int) -> tuple:
    return board.fen(), board.chess960, tuple(move.uci() for move in board.move_stack), depth, multipv


def record(engine: Engine, pgn: str, opts: AnalysisOptions, time_cap: float, nodes: int | None) -> dict:
    calls = []
    game = parse_pgn(pgn)
    total_nodes = 0
    traced = None
    original = None

    def evaluate(board, depth, multipv):
        nonlocal total_nodes, traced, original
        # Observe the wrapper's real UCI calls without changing its search or output.
        if engine._engine is not traced:
            traced = engine._engine
            original = traced.analyse

            def observe(*args, **kwargs):
                nonlocal total_nodes
                infos = original(*args, **kwargs)
                total_nodes += max((info.get("nodes", 0) for info in infos), default=0)
                return infos

            traced.analyse = observe
        previous_nodes = total_nodes
        start = time.perf_counter()
        lines = engine.analyse(board, depth, multipv, time_cap=time_cap, nodes=nodes)
        elapsed = (time.perf_counter() - start) * 1000
        calls.append({
            "key": request_key(board, depth, multipv), "lines": [line.to_dict() for line in lines],
            "engine_ms": elapsed, "nodes": total_nodes - previous_nodes,
        })
        return lines

    start = time.perf_counter()
    try:
        result = analyze_parsed(game, evaluate, opts)
    finally:
        if traced is not None:
            traced.analyse = original
    wall = (time.perf_counter() - start) * 1000
    return {"pgn": pgn, "calls": calls, "result": result, "wall_ms": wall}


def replay(recording: dict, opts: AnalysisOptions, parsed=None) -> dict:
    requests = iter(recording["calls"])

    def evaluate(board, depth, multipv):
        expected = next(requests)
        key = request_key(board, depth, multipv)
        stored = expected["key"]
        assert key == (stored[0], stored[1], tuple(stored[2]), stored[3], stored[4]), "Engine request changed"
        return [Line.from_dict(line) for line in expected["lines"]]

    result = analyze_parsed(parsed or parse_pgn(recording["pgn"]), evaluate, opts)
    assert next(requests, None) is None, "An engine request was dropped"
    assert {k: v for k, v in result.items() if k != "duration_ms"} == {
        k: v for k, v in recording["result"].items() if k != "duration_ms"
    }, "Analysis output changed"
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pgn", type=Path)
    parser.add_argument("--depth", type=int, default=16)
    parser.add_argument("--multipv", type=int, default=2)
    parser.add_argument("--time-cap", type=float, default=5)
    parser.add_argument("--nodes", type=int)
    parser.add_argument("--threads", type=int, default=1)
    parser.add_argument("--hash-mb", type=int, default=64)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--record", type=Path)
    parser.add_argument("--replay", type=Path)
    parser.add_argument("--profile", type=Path)
    args = parser.parse_args()
    if min(args.depth, args.multipv, args.threads, args.hash_mb, args.repeat, args.time_cap) <= 0:
        parser.error("search budgets and repetitions must be positive")
    if args.nodes is not None and args.nodes <= 0:
        parser.error("--nodes must be positive")
    opts = AnalysisOptions(args.depth, args.multipv)
    if args.replay:
        capture = json.loads(args.replay.read_text(encoding="utf-8"))
        opts = AnalysisOptions(**capture["options"])
    else:
        if args.pgn:
            pgns = split_games(args.pgn.read_text(encoding="utf-8"))
        else:
            fixtures = ROOT / "tests" / "fixtures" / "chesscom"
            pgns = [
                json.loads((fixtures / file).read_text(encoding="utf-8"))["games"][index]["pgn"]
                for file, index in (("2026-10.json", 0), ("2026-11.json", 1))
            ]
        if not pgns:
            parser.error("PGN input contains no games")
        start = time.perf_counter()
        engine = Engine(threads=args.threads, hash_mb=args.hash_mb)
        startup_ms = (time.perf_counter() - start) * 1000
        try:
            capture = {
                "engine": engine.name, "startup_ms": startup_ms, "options": vars(opts),
                "time_cap": args.time_cap, "nodes": args.nodes, "threads": args.threads, "hash_mb": args.hash_mb,
                "games": [record(engine, pgn, opts, args.time_cap, args.nodes) for pgn in pgns],
            }
        finally:
            engine.close()
        if args.record:
            args.record.write_text(json.dumps(capture), encoding="utf-8")

    parsed = [parse_pgn(game["pgn"]) for game in capture["games"]]

    def replay_all():
        return [replay(game, opts, game_parsed) for game, game_parsed in zip(capture["games"], parsed, strict=True)]

    replay_all()
    timings = []
    for _ in range(args.repeat):
        start = time.perf_counter()
        replay_all()
        timings.append((time.perf_counter() - start) * 1000)
    if args.profile:
        profiler = cProfile.Profile()
        profiler.runcall(replay_all)
        profiler.dump_stats(args.profile)
    samples = []
    for game in capture["games"]:
        calls = game["calls"]
        engine_ms = sum(call["engine_ms"] for call in calls)
        samples.append({
            "plies": len(game["result"]["moves"]), "wall_ms": round(game["wall_ms"], 2),
            "engine_ms": round(engine_ms, 2), "python_overhead_ms": round(game["wall_ms"] - engine_ms, 2),
            "engine_calls": len(calls), "threat_calls": len(calls) - len(game["result"]["moves"]) - 1,
            "nodes": sum(call["nodes"] for call in calls),
            "achieved_depths": sorted({line["depth"] for call in calls for line in call["lines"]}),
        })
    result = {
        "python": platform.python_version(), "platform": platform.system(), "engine": capture["engine"],
        "options": capture["options"], "time_cap": capture["time_cap"], "node_cap": capture["nodes"],
        "threads": capture["threads"], "hash_mb": capture["hash_mb"],
        "startup_ms": round(capture["startup_ms"], 2), "samples": samples,
        "replay_ms": timings, "replay_median_ms": round(statistics.median(timings), 2), "output_equal": True,
    }
    print(json.dumps(result, indent=2))
    if args.out:
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
