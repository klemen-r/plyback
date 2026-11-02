"""Shared helpers for API routes."""

from __future__ import annotations

from fastapi import HTTPException, Request

from ..services import store


def S(request: Request):
    """The application's service container."""
    return request.app.state.services


def account_or_404(s, account_id: int) -> dict:
    acc = store.account_by_id(s.db, account_id)
    if acc is None:
        raise HTTPException(404, "Account not found")
    return acc
