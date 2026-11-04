# Stockfish installer verification

This records the retained Python baseline installer. Native setup now uses
`scripts/install_stockfish.mjs`; its mocked safety regressions run in CI on Linux and Windows.
The memory and timing numbers below do not describe the new Node setup helper.

Measured on 2026-11-02 using Python 3.14.3 on Windows 11 AMD64. The test used the
[official Stockfish 19 Windows universal ZIP](https://github.com/official-stockfish/Stockfish/releases/download/sf_19/stockfish-windows-x86-64-universal.zip),
81,431,614 bytes, verified against the release asset's SHA-256 digest. Raw runtime
information and every sample are in [installer-performance-results.json](installer-performance-results.json).

## Extraction memory

The previous installer retained the downloaded ZIP and every decompressed member
in memory before choosing a binary. The replacement downloads to a staging file,
selects members using archive metadata, and copies only the binary and notices in
1 MiB chunks. Download and extraction sizes are bounded.

Three paired samples alternated the source-equivalent previous extraction method
and the new `_extract_binary` function against the same local ZIP. Each sample
used a fresh output directory and `gc.collect()` before `tracemalloc.start()`;
`time.perf_counter()` measured extraction. The reference reads the complete archive
into `BytesIO`, reads every non-directory member into a `(filename, bytes)` list,
selects the largest Stockfish binary using the previous suffix exclusions, and
writes it to disk. The new function also writes the license and author notices.

| Extraction method | Median peak Python allocation | Median elapsed time |
|---|---:|---:|
| Previous read-all reference | 363.171 MiB | 681.9 ms |
| Staged, streamed extraction | 4.912 MiB | 582.2 ms |

Peak traced Python allocation fell **98.65%**. All six extracted binaries had the
same SHA-256, `45bc8e4969147db9c2eb533810637994619bff0eacc81ccfd9854394901bcbd0`.
These measurements exclude downloading, UCI verification and publication. They
measure Python allocations rather than total process RSS. Elapsed times varied
between runs while other engineering checks were active; they do not establish a
general installation speedup.

## Installation integrity and failure recovery

An independent real installation into a temporary directory downloaded Stockfish
19, matched the official archive digest, completed the UCI handshake, and retained
the 35,149-byte GPL license and a manifest pointing to the exact `sf_19` source.
That single smoke run took 4.29 seconds and peaked at 5.13 MiB of traced Python
allocation; it is not a repeated network benchmark. The temporary installation was
removed after inspection; the user's working engine was not replaced.

The installer now verifies a staged engine's successful exit, identity and `uciok`
before publishing it. A complete version directory retains the binary, available
`AUTHORS` and `README.md`, `Copying.txt`, the archive hash, digest-verification
status and exact source URL. `stockfish.path` and the compatibility executable are
replaced atomically after verification. The old executable and path remain intact
on checksum or UCI failure. Older version directories remain available for rollback.

This preserves the license and source directions described by the
[upstream distribution terms](https://github.com/official-stockfish/Stockfish#terms-of-use).
Release metadata does not always provide a digest: a missing digest is recorded as
unverified rather than described as a verified download.

The installer regression suite covers failed upgrades, notices and provenance,
complete UCI verification, archive links and path traversal, download limits and
rejection of asset URLs outside the official repository:

```powershell
.\.venv\Scripts\python.exe -m pytest backend/tests/test_installer.py -o addopts= -q
```

Eight tests passed. Linux/macOS installation and native binaries were not executed
on this Windows host; TAR extraction was exercised with a synthetic archive.
