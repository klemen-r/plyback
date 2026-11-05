import hashlib
import importlib.util
import io
import json
import subprocess
import tarfile
import zipfile
from pathlib import Path
from types import SimpleNamespace

import pytest

SPEC = importlib.util.spec_from_file_location("install_stockfish", Path(__file__).parents[2] / "scripts/install_stockfish.py")
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)

ASSET_URL = "https://github.com/official-stockfish/Stockfish/releases/download/sf_test/stockfish-test.zip"
LICENSE_URL = "https://raw.githubusercontent.com/official-stockfish/Stockfish/sf_test/Copying.txt"


@pytest.mark.parametrize("token", [None, "", "fixture-github-token"])
def test_metadata_auth_is_optional_and_scoped_to_release_api(monkeypatch, token, tmp_path):
    if token is None:
        monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    else:
        monkeypatch.setenv("GITHUB_TOKEN", token)
    requests = []

    def open_request(req, timeout):
        assert timeout == 120
        requests.append((req.full_url, req.get_header("Authorization")))
        return io.BytesIO(b"official source")

    def build_opener(handler):
        assert isinstance(handler, installer._SourceRedirectHandler)
        return SimpleNamespace(open=open_request)

    monkeypatch.setattr(installer.urllib.request, "build_opener", build_opener)
    monkeypatch.setattr(installer.urllib.request, "urlopen", open_request)
    assert installer._fetch(installer.RELEASE_API) == b"official source"
    assert installer._fetch(LICENSE_URL) == b"official source"
    installer._download(ASSET_URL, tmp_path / "archive", None)
    assert requests == [
        (installer.RELEASE_API, f"Bearer {token}" if token else None),
        (LICENSE_URL, None),
        (ASSET_URL, None),
    ]


@pytest.mark.parametrize("url", [
    "http://api.github.com/repos/official-stockfish/Stockfish/releases/latest",
    "https://api.github.com/repos/another-owner/Stockfish/releases/latest",
    "https://raw.githubusercontent.com/another-owner/Stockfish/main/Copying.txt",
    "https://raw.githubusercontent.com/official-stockfish/Stockfish/main%2F..%2F..%2Fother/Copying.txt",
    "https://raw.githubusercontent.com/official-stockfish/Stockfish/main%5C..%5Cother/Copying.txt",
    "https://user:password@raw.githubusercontent.com/official-stockfish/Stockfish/main/Copying.txt",
    "https://raw.githubusercontent.com:8443/official-stockfish/Stockfish/main/Copying.txt",
    LICENSE_URL + "#fragment",
])
def test_metadata_rejects_untrusted_source_before_network(monkeypatch, url):
    def unexpected_request(*args, **kwargs):
        raise AssertionError("Unexpected network request")

    monkeypatch.setenv("GITHUB_TOKEN", "fixture-secret")
    monkeypatch.setattr(installer.urllib.request, "build_opener", unexpected_request)
    with pytest.raises(RuntimeError, match="outside official Stockfish"):
        installer._fetch(url)


def test_metadata_redirects_never_forward_token_to_other_urls():
    request = installer.urllib.request.Request(installer.RELEASE_API)
    request.add_unredirected_header("Authorization", "Bearer fixture-secret")
    handler = installer._SourceRedirectHandler()
    for target in [LICENSE_URL, ASSET_URL, installer.RELEASE_API + "/other", "https://api.github.com.evil.invalid/"]:
        with pytest.raises(RuntimeError, match="Unexpected release"):
            handler.redirect_request(request, None, 302, "Found", {}, target)
    redirected = handler.redirect_request(request, None, 302, "Found", {}, installer.RELEASE_API)
    assert redirected.get_header("Authorization") == "Bearer fixture-secret"
    modified = handler.redirect_request(request, None, 302, "Found", {}, installer.RELEASE_API + "?redirected=1")
    assert modified.get_header("Authorization") is None
    license_request = installer.urllib.request.Request(LICENSE_URL)
    redirected_license = handler.redirect_request(license_request, None, 302, "Found", {}, LICENSE_URL)
    assert redirected_license.get_header("Authorization") is None


def test_invalid_token_diagnostics_do_not_include_secret(monkeypatch):
    monkeypatch.setenv("GITHUB_TOKEN", "fixture-secret\ninvalid")
    with pytest.raises(RuntimeError, match="GITHUB_TOKEN") as error:
        installer._fetch(installer.RELEASE_API)
    assert "fixture-secret" not in str(error.value)


def test_metadata_remains_bounded(monkeypatch):
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    monkeypatch.setattr(installer, "MAX_NOTICE_BYTES", 8)
    opener = SimpleNamespace(open=lambda *a, **kw: io.BytesIO(b"123456789"))
    monkeypatch.setattr(installer.urllib.request, "build_opener", lambda *args: opener)
    with pytest.raises(RuntimeError, match="size limit"):
        installer._fetch(installer.RELEASE_API)


def archive_bytes():
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("stockfish/stockfish.exe", b"engine")
        archive.writestr("stockfish/Copying.txt", b"GPL version 3")
        archive.writestr("stockfish/AUTHORS", b"Stockfish contributors")
        archive.writestr("stockfish/README.md", b"Stockfish source and notices")
        archive.writestr("stockfish/huge.nnue", b"not extracted")
        archive.writestr("../../outside", b"not extracted")
    return buffer.getvalue()


def configure_release(monkeypatch, data, digest=None):
    release = {
        "tag_name": "sf_test",
        "name": "Test release",
        "assets": [{"name": "stockfish-test.zip", "browser_download_url": ASSET_URL, "digest": digest}],
    }
    monkeypatch.setattr(installer, "_fetch", lambda url: json.dumps(release).encode())
    monkeypatch.setattr(installer, "_asset_candidates", lambda: ["stockfish-test", "stockfish-test"])
    monkeypatch.setattr(installer.platform, "system", lambda: "Windows")
    monkeypatch.setattr(installer.urllib.request, "urlopen", lambda *a, **kw: io.BytesIO(data))


def existing_install(dest):
    dest.mkdir()
    binary = dest / "stockfish.exe"
    binary.write_bytes(b"working original")
    (dest / "stockfish.path").write_text(str(binary))
    return binary


@pytest.mark.parametrize("failure", ["digest", "uci"])
def test_failed_upgrade_preserves_working_installation(tmp_path, monkeypatch, failure):
    dest = tmp_path / "engine"
    original = existing_install(dest)
    data = archive_bytes()
    digest = "sha256:" + ("0" * 64 if failure == "digest" else hashlib.sha256(data).hexdigest())
    configure_release(monkeypatch, data, digest)

    def verify(binary):
        raise RuntimeError("invalid UCI response")

    monkeypatch.setattr(installer, "_verify", verify)
    with pytest.raises(SystemExit, match="Could not install Stockfish"):
        installer.install(dest, force=True)
    assert original.read_bytes() == b"working original"
    assert (dest / "stockfish.path").read_text() == str(original)
    assert not list(dest.glob(".stockfish-*"))
    assert not (dest / "versions").exists()


def test_install_publishes_verified_binary_and_exact_source_notices(tmp_path, monkeypatch):
    dest = tmp_path / "engine"
    existing_install(dest)
    data = archive_bytes()
    configure_release(monkeypatch, data, "sha256:" + hashlib.sha256(data).hexdigest())
    seen = []

    def verify(binary):
        assert binary.read_bytes() == b"engine"
        assert (dest / "stockfish.exe").read_bytes() == b"working original"
        seen.append(binary)
        return "Test Fish"

    monkeypatch.setattr(installer, "_verify", verify)
    binary = installer.install(dest, force=True)
    assert len(seen) == 1
    assert binary.read_bytes() == (dest / "stockfish.exe").read_bytes() == b"engine"
    assert Path((dest / "stockfish.path").read_text()) == binary
    manifest = json.loads((binary.parent / "manifest.json").read_text())
    assert manifest["archive_sha256"] == hashlib.sha256(data).hexdigest()
    assert manifest["release_digest_verified"]
    assert manifest["source_url"].endswith("/refs/tags/sf_test.tar.gz")
    assert (binary.parent / "Copying.txt").read_bytes() == b"GPL version 3"
    assert (binary.parent / "AUTHORS").read_bytes() == b"Stockfish contributors"
    assert not (tmp_path / "outside").exists()
    assert not (binary.parent / "huge.nnue").exists()
    assert not list(dest.glob(".stockfish-*"))


@pytest.mark.parametrize("code,stdout", [(1, "id name Fish\nuciok\n"), (0, "id name Fish\n")])
def test_verification_requires_complete_successful_uci(monkeypatch, code, stdout):
    monkeypatch.setattr(installer.subprocess, "run", lambda *a, **kw: subprocess.CompletedProcess(a, code, stdout, ""))
    with pytest.raises(RuntimeError, match="did not respond"):
        installer._verify(Path("stockfish"))


def test_tar_extraction_ignores_links_and_path_traversal(tmp_path, monkeypatch):
    monkeypatch.setattr(installer.platform, "system", lambda: "Linux")
    path = tmp_path / "engine.tar.gz"
    with tarfile.open(path, "w:gz") as archive:
        link = tarfile.TarInfo("stockfish/stockfish-link")
        link.type = tarfile.SYMTYPE
        link.linkname = "/etc/passwd"
        archive.addfile(link)
        for name, content in [("../../stockfish-linux", b"engine"), ("stockfish/Copying.txt", b"license")]:
            member = tarfile.TarInfo(name)
            member.size = len(content)
            archive.addfile(member, io.BytesIO(content))
    binary = installer._extract_binary(path, path.name, tmp_path / "out")
    assert binary.read_bytes() == b"engine"
    assert not (tmp_path / "stockfish-linux").exists()
    assert (binary.parent / "Copying.txt").read_bytes() == b"license"


def test_download_rejects_nonofficial_asset_before_network(tmp_path, monkeypatch):
    def unexpected_request(*args, **kwargs):
        raise AssertionError("untrusted URLs must not reach the network")

    monkeypatch.setattr(installer.urllib.request, "urlopen", unexpected_request)
    with pytest.raises(RuntimeError, match="outside the official"):
        installer._download("https://evil.example/stockfish.zip", tmp_path / "archive", None)


def test_download_is_bounded_and_does_not_read_archive_into_memory(tmp_path, monkeypatch):
    class Response(io.BytesIO):
        def read(self, size=-1):
            assert 0 < size <= installer.CHUNK_SIZE
            return super().read(size)

    monkeypatch.setattr(installer.urllib.request, "urlopen", lambda *a, **kw: Response(b"123456789"))
    monkeypatch.setattr(installer, "MAX_ARCHIVE_BYTES", 8)
    with pytest.raises(RuntimeError, match="download size limit"):
        installer._download(ASSET_URL, tmp_path / "archive", None)
