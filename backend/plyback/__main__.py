"""Command-line entry point.

python -m plyback                 start the server
python -m plyback set-password    set or change the access password
python -m plyback clear-password  remove the access password (local use only)
python -m plyback backup PATH     save a consistent database snapshot
"""

from __future__ import annotations

import getpass
import sys
from pathlib import Path

from . import config, security
from .db import Database
from .main import adopt_legacy_database, backup_database, run, setup_logging


def _db() -> Database:
    setup_logging("WARNING")
    adopt_legacy_database()
    db = Database(config.DB_PATH)
    db.migrate()
    return db


def main(argv: list[str] | None = None) -> None:
    args = sys.argv[1:] if argv is None else argv
    command = args[0] if args else "serve"
    if command == "serve":
        run()
    elif command == "set-password":
        first = getpass.getpass("New password: ")
        if first != getpass.getpass("Repeat password: "):
            raise SystemExit("Passwords do not match.")
        try:
            security.validate_new_password(first)
        except ValueError as exc:
            raise SystemExit(str(exc)) from exc
        db = _db()
        try:
            security.set_password(db, first)
        finally:
            db.close_all()
        print("Password set. Existing sessions were signed out.")
    elif command == "clear-password":
        db = _db()
        try:
            security.set_password(db, None)
        finally:
            db.close_all()
        print("Password removed. Plyback is open to anyone who can reach it locally.")
    elif command == "backup" and len(args) == 2:
        source = config.DB_PATH if config.DB_PATH.exists() else config.LEGACY_DB_PATH
        backup_database(source, Path(args[1]))
        print(f"Database snapshot saved to {Path(args[1]).resolve()}")
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
