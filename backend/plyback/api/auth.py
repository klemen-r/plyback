"""Sign-in, sign-out and password management."""

from __future__ import annotations

from fastapi import APIRouter, HTTPException, Request, Response
from pydantic import BaseModel, Field

from .. import config, security
from .deps import S

router = APIRouter(prefix="/auth")


class LoginIn(BaseModel):
    password: str = Field(..., max_length=256)


class PasswordIn(BaseModel):
    current: str | None = Field(None, max_length=256)
    password: str | None = Field(None, max_length=256)


def _session_state(request: Request) -> dict:
    db = S(request).db
    required = security.auth_required(db)
    authed = (not required) or security.session_valid(db, request.cookies.get(security.SESSION_COOKIE))
    return {"authenticated": authed, "password_set": required, "auth_required": required}


@router.get("/session")
def session(request: Request):
    return _session_state(request)


@router.post("/login")
def login(request: Request, response: Response, body: LoginIn):
    db = S(request).db
    stored = security.password_hash(db)
    if stored is None:
        return _session_state(request)
    if not security.verify_password(body.password, stored):
        raise HTTPException(401, "That password is not correct.")
    token = security.create_session(db, request.headers.get("user-agent"))
    security.set_session_cookie(response, request, token)
    return {"authenticated": True, "password_set": True, "auth_required": True}


@router.post("/logout")
def logout(request: Request, response: Response):
    security.end_session(S(request).db, request.cookies.get(security.SESSION_COOKIE))
    response.delete_cookie(security.SESSION_COOKIE, path="/")
    return {"ok": True}


@router.post("/password")
def change_password(request: Request, response: Response, body: PasswordIn):
    """Set, change or remove the access password. Changing or removing needs the current one."""
    db = S(request).db
    stored = security.password_hash(db)
    if stored is not None and not security.verify_password(body.current or "", stored):
        raise HTTPException(403, "The current password is not correct.")
    if body.password is None:
        if not security.is_loopback_host(config.HOST):
            raise HTTPException(400, "Stop network access and restart with a loopback bind before removing the password.")
        security.set_password(db, None)
        response.delete_cookie(security.SESSION_COOKIE, path="/")
        return {"authenticated": True, "password_set": False, "auth_required": False}
    try:
        security.validate_new_password(body.password)
    except ValueError as exc:
        raise HTTPException(400, str(exc)) from exc
    security.set_password(db, body.password)
    token = security.create_session(db, request.headers.get("user-agent"))
    security.set_session_cookie(response, request, token)
    return {"authenticated": True, "password_set": True, "auth_required": True}
