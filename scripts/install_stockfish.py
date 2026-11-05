"""Download the latest stable Stockfish release from the official GitHub repository.

Usage:  python scripts/install_stockfish.py [--dest DIR] [--force]

The binary is placed in ``data/engine/`` (or ``--dest``) and its path is written to
``data/engine/stockfish.path`` so the backend can find it.  Only the official
``official-stockfish/Stockfish`` release assets are ever downloaded.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse
import urllib.request
import uuid
import zipfile
from pathlib import Path

RELEASE_API = "https://api.github.com/repos/official-stockfish/Stockfish/releases/latest"
ROOT = Path(__file__).resolve().parent.parent
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_NOTICE_BYTES = 2 * 1024 * 1024
CHUNK_SIZE = 1024 * 1024


def _asset_candidates() -> list[str]:
    """Return asset-name substrings in order of preference for this machine."""
    system = platform.system().lower()
    machine = platform.machine().lower()
    arm = machine in ("arm64", "aarch64")
    if system == "windows":
        osname = "windows"
    elif system == "darwin":
        return ["macos-universal", "macos-m1-apple-silicon" if arm else "macos-x86-64-avx2", "macos-x86-64"]
    else:
        osname = "ubuntu" if not arm else "linux"
    if arm:
        return [f"{osname}-arm64-universal", "linux-arm64-universal", f"{osname}-armv8"]
    # Stockfish 18+ ships "universal" builds that pick the best code path at runtime.
    return [
        f"{osname}-x86-64-universal",
        "linux-x86-64-universal" if osname == "ubuntu" else f"{osname}-x86-64-universal",
        f"{osname}-x86-64-avx2",
        f"{osname}-x86-64-sse41-popcnt",
        f"{osname}-x86-64",
    ]


def _official_license_url(url: str) -> bool:
    parsed = urllib.parse.urlsplit(url)
    decoded_path = urllib.parse.unquote(parsed.path)
    return (
        parsed.scheme == "https"
        and parsed.netloc == "raw.githubusercontent.com"
        and not parsed.query
        and not parsed.fragment
        and parsed.path.startswith("/official-stockfish/Stockfish/")
        and parsed.path.endswith("/Copying.txt")
        and ".." not in decoded_path.split("/")
        and "\\" not in decoded_path
        and not any(ord(char) < 32 or ord(char) == 127 for char in decoded_path)
    )


class _SourceRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Keep metadata and license requests at their validated source path."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        original = urllib.parse.urlsplit(req.full_url)
        target = urllib.parse.urlsplit(newurl)
        if (
            (target.scheme, target.netloc, target.path) != (original.scheme, original.netloc, original.path)
            or target.fragment
        ):
            raise RuntimeError("Unexpected release metadata or license redirect")
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        # urllib omits unredirected headers by default. Reattach only for the
        # exact API endpoint, never a license URL or modified redirect URL.
        token = req.get_header("Authorization")
        if redirected is not None and newurl == RELEASE_API and token:
            redirected.add_unredirected_header("Authorization", token)
        return redirected


def _fetch(url: str) -> bytes:
    if url != RELEASE_API and not _official_license_url(url):
        raise RuntimeError("Metadata URL is outside official Stockfish sources")
    req = urllib.request.Request(  # noqa: S310 - exact official HTTPS endpoint checked above
        url, headers={"User-Agent": "plyback-installer", "Accept": "application/octet-stream, application/json"}
    )
    token = os.environ.get("GITHUB_TOKEN") if url == RELEASE_API else None
    if token:
        if any(ord(char) < 33 or ord(char) > 126 for char in token):
            raise RuntimeError("GITHUB_TOKEN must contain only visible ASCII characters")
        req.add_unredirected_header("Authorization", f"Bearer {token}")
    opener = urllib.request.build_opener(_SourceRedirectHandler())
    with opener.open(req, timeout=120) as r:
        data = r.read(MAX_NOTICE_BYTES + 1)
    if len(data) > MAX_NOTICE_BYTES:
        raise RuntimeError("Release metadata or notice exceeds the size limit")
    return data


def _download(url: str, dest: Path, expected_digest: str | None) -> str:
    parsed = urllib.parse.urlsplit(url)
    if (
        parsed.scheme != "https"
        or parsed.netloc != "github.com"
        or not parsed.path.startswith("/official-stockfish/Stockfish/releases/download/")
    ):
        raise RuntimeError("Release asset is outside the official Stockfish repository")
    if expected_digest and (
        not expected_digest.startswith("sha256:")
        or len(expected_digest) != 71
        or any(c not in "0123456789abcdef" for c in expected_digest[7:].lower())
    ):
        raise RuntimeError("Unsupported or malformed release digest")
    digest = hashlib.sha256()
    total = 0
    req = urllib.request.Request(url, headers={"User-Agent": "plyback-installer"})  # noqa: S310 - allowlisted HTTPS asset
    with urllib.request.urlopen(req, timeout=120) as response, dest.open("wb") as output:  # noqa: S310
        while chunk := response.read(CHUNK_SIZE):
            total += len(chunk)
            if total > MAX_ARCHIVE_BYTES:
                raise RuntimeError("Stockfish archive exceeds the download size limit")
            output.write(chunk)
            digest.update(chunk)
    actual = digest.hexdigest()
    if expected_digest and actual != expected_digest[7:].lower():
        raise RuntimeError("Stockfish archive SHA-256 mismatch")
    return actual


def _copy_member(source, target: Path, limit: int) -> None:
    total = 0
    with source, target.open("wb") as output:
        while chunk := source.read(CHUNK_SIZE):
            total += len(chunk)
            if total > limit:
                raise RuntimeError("Stockfish archive member exceeds the extraction size limit")
            output.write(chunk)


def _extract_binary(archive: Path, name: str, dest: Path) -> Path:
    dest.mkdir(parents=True, exist_ok=True)
    exe_suffix = ".exe" if platform.system() == "Windows" else ""
    target = dest / f"stockfish{exe_suffix}"
    notices = {"copying.txt": "Copying.txt", "authors": "AUTHORS", "readme.md": "README.md"}

    def binary_candidate(filename: str) -> bool:
        base = Path(filename).name.lower()
        return base.startswith("stockfish") and (base.endswith(".exe") if exe_suffix else "." not in base)

    if name.endswith(".zip"):
        with zipfile.ZipFile(archive) as z:
            members = [m for m in z.infolist() if not m.is_dir() and not stat.S_ISLNK(m.external_attr >> 16)]
            binaries = [m for m in members if binary_candidate(m.filename)]
            if not binaries:
                raise RuntimeError(f"No stockfish binary found inside {name}")
            selected = max(binaries, key=lambda m: m.file_size)
            if selected.file_size > MAX_BINARY_BYTES:
                raise RuntimeError("Stockfish binary exceeds the extraction size limit")
            _copy_member(z.open(selected), target, MAX_BINARY_BYTES)
            for member in members:
                notice = notices.get(Path(member.filename).name.lower())
                if notice and member.file_size <= MAX_NOTICE_BYTES and not (dest / notice).exists():
                    _copy_member(z.open(member), dest / notice, MAX_NOTICE_BYTES)
    else:
        with tarfile.open(archive, mode="r:*") as t:
            members = [m for m in t.getmembers() if m.isfile()]
            binaries = [m for m in members if binary_candidate(m.name)]
            if not binaries:
                raise RuntimeError(f"No stockfish binary found inside {name}")
            selected = max(binaries, key=lambda m: m.size)
            if selected.size > MAX_BINARY_BYTES:
                raise RuntimeError("Stockfish binary exceeds the extraction size limit")
            _copy_member(t.extractfile(selected), target, MAX_BINARY_BYTES)
            for member in members:
                notice = notices.get(Path(member.name).name.lower())
                if notice and member.size <= MAX_NOTICE_BYTES and not (dest / notice).exists():
                    _copy_member(t.extractfile(member), dest / notice, MAX_NOTICE_BYTES)
    target.chmod(target.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return target


def _verify(binary: Path) -> str:
    out = subprocess.run([str(binary)], input="uci\nquit\n", capture_output=True, text=True, timeout=30, check=False)
    lines = out.stdout.splitlines()
    for line in lines:
        if out.returncode == 0 and "uciok" in lines and line.startswith("id name "):
            ident = line[len("id name ") :].strip()
            if ident:
                return ident
    raise RuntimeError(f"{binary} did not respond to UCI: {out.stdout[:200]} {out.stderr[:200]}")


def install(dest: Path, force: bool = False) -> Path:
    dest = dest.resolve()
    path_file = dest / "stockfish.path"
    if path_file.exists() and not force:
        existing = Path(path_file.read_text().strip())
        if existing.exists():
            try:
                print(f"Stockfish already installed: {_verify(existing)} at {existing}")
                return existing
            except (OSError, RuntimeError, subprocess.SubprocessError) as exc:
                print(f"Existing Stockfish is unavailable ({exc}); attempting a replacement.")
    print("Querying latest Stockfish release ...")
    release = json.loads(_fetch(RELEASE_API))
    assets = {a["name"]: a for a in release.get("assets", [])}
    errors: list[str] = []
    tried: set[str] = set()
    for cand in _asset_candidates():
        for name, asset in assets.items():
            if name not in tried and cand in name and name.endswith((".zip", ".tar", ".tar.gz", ".tgz")):
                tried.add(name)
                print(f"Downloading {name} ({release.get('name')}) ...")
                try:
                    dest.mkdir(parents=True, exist_ok=True)
                    with tempfile.TemporaryDirectory(prefix=".stockfish-", dir=dest) as temporary:
                        stage = Path(temporary)
                        archive = stage / "download"
                        url = asset["browser_download_url"]
                        digest = _download(url, archive, asset.get("digest"))
                        output = stage / "engine"
                        binary = _extract_binary(archive, name, output)
                        ident = _verify(binary)
                        tag = release.get("tag_name")
                        if not isinstance(tag, str) or not tag:
                            raise RuntimeError("Release is missing its source tag")
                        quoted_tag = urllib.parse.quote(tag, safe="")
                        if not (output / "Copying.txt").exists():
                            license_url = f"https://raw.githubusercontent.com/official-stockfish/Stockfish/{quoted_tag}/Copying.txt"
                            (output / "Copying.txt").write_bytes(_fetch(license_url))
                        manifest = {
                            "engine": ident,
                            "release": tag,
                            "asset": name,
                            "asset_url": url,
                            "archive_sha256": digest,
                            "release_digest_verified": bool(asset.get("digest")),
                            "source_url": f"https://github.com/official-stockfish/Stockfish/archive/refs/tags/{quoted_tag}.tar.gz",
                            "license": "GPL-3.0",
                        }
                        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
                        notice = stage / "STOCKFISH-NOTICES.txt"
                        notice.write_text(
                            "Stockfish is distributed under GPL-3.0. Each installed version, its Copying.txt, "
                            "available AUTHORS and README.md notices, and manifest.json with the exact release "
                            "source URL are retained under versions/. stockfish.path identifies the selected "
                            "backend binary; stockfish[.exe] is a compatibility link or copy.\n",
                            encoding="utf-8",
                        )
                        notice.replace(dest / notice.name)
                        versions = dest / "versions"
                        versions.mkdir(exist_ok=True)
                        installed = versions / uuid.uuid4().hex
                        output.replace(installed)
                        installed_binary = installed / binary.name
                        alias = stage / binary.name
                        try:
                            os.link(installed_binary, alias)
                        except OSError:
                            shutil.copy2(installed_binary, alias)
                        alias.replace(dest / binary.name)
                        staged_pointer = stage / "stockfish.path"
                        staged_pointer.write_text(str(installed_binary), encoding="utf-8")
                        staged_pointer.replace(path_file)
                        binary = installed_binary
                except (OSError, RuntimeError, subprocess.SubprocessError, zipfile.BadZipFile, tarfile.TarError) as exc:
                    errors.append(f"{name}: {exc}")
                    continue
                print(f"Installed {ident} -> {binary}")
                return binary
    raise SystemExit(
        "Could not install Stockfish automatically.\n"
        + "\n".join(errors)
        + "\nDownload it from https://stockfishchess.org/download/ and set PLYBACK_STOCKFISH."
    )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dest", default=str(ROOT / "data" / "engine"))
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()
    if shutil.which("stockfish") and not args.force and not os.environ.get("STOCKFISH_FORCE_DOWNLOAD"):
        print(f"Note: a system stockfish exists at {shutil.which('stockfish')}; installing a local copy anyway.")
    install(Path(args.dest), args.force)


if __name__ == "__main__":
    sys.exit(main())
