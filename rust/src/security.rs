//! Authentication format remains compatible with the Python baseline. Secrets and
//! session tokens never appear in API errors or logs.
use crate::{api_error, db::Database, now};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::{TryRng, rngs::SysRng};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Instant};
use subtle::ConstantTimeEq;

pub const SESSION_COOKIE: &str = "plyback_session";
pub const SESSION_TTL: i64 = 30 * 86400;
pub const PASSWORD_KEY: &str = "auth_password_hash";
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https://images.chesscomfiles.com https://www.chess.com; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

pub fn validate_password(password: &str) -> anyhow::Result<()> {
    match password.chars().count() {
        0..8 => Err(api_error(400, "Use at least 8 characters.")),
        257.. => Err(api_error(400, "That password is too long.")),
        _ => Ok(()),
    }
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut salt = [0; 16];
    // Straight from the operating system; a failure is an error, never a weak salt.
    SysRng.try_fill_bytes(&mut salt)?;
    let mut digest = [0; 32];
    scrypt::scrypt(
        password.as_bytes(),
        &salt,
        &scrypt::Params::new(14, 8, 1)?,
        &mut digest,
    )?;
    Ok(format!(
        "scrypt$16384$8$1${}${}",
        STANDARD.encode(salt),
        STANDARD.encode(digest)
    ))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    if password.chars().count() > 256 || stored.len() > 256 {
        return false;
    }
    let parts: Vec<_> = stored.split('$').collect();
    // The old runtime generated exactly these parameters. Reject unbounded KDF
    // costs from corrupted data instead of allocating arbitrary amounts of RAM.
    if parts.len() != 6 || parts[..4] != ["scrypt", "16384", "8", "1"] {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (STANDARD.decode(parts[4]), STANDARD.decode(parts[5])) else {
        return false;
    };
    if salt.len() != 16 || expected.len() != 32 {
        return false;
    }
    let mut actual = [0; 32];
    let Ok(params) = scrypt::Params::new(14, 8, 1) else {
        return false;
    };
    if scrypt::scrypt(password.as_bytes(), &salt, &params, &mut actual).is_err() {
        return false;
    }
    bool::from(actual.ct_eq(expected.as_slice()))
}

pub fn password_hash(db: &Database) -> anyhow::Result<Option<String>> {
    Ok(db
        .one(
            "SELECT value FROM settings WHERE key=?",
            &[json!(PASSWORD_KEY)],
        )?
        .and_then(|row| row["value"].as_str().map(str::to_owned)))
}

pub fn set_password(db: &Database, password: Option<&str>) -> anyhow::Result<()> {
    let hashed = password.map(hash_password).transpose()?;
    db.transaction(|tx| {
        if let Some(value) = hashed {
            tx.execute("INSERT INTO settings(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", rusqlite::params![PASSWORD_KEY,value])?;
        } else {
            tx.execute("DELETE FROM settings WHERE key=?", [PASSWORD_KEY])?;
        }
        tx.execute("DELETE FROM sessions", [])?;
        Ok(())
    })
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
pub fn create_session(db: &Database, user_agent: &str) -> anyhow::Result<String> {
    db.transaction(|tx| insert_session(tx, user_agent))
}

fn insert_session(tx: &rusqlite::Transaction<'_>, user_agent: &str) -> anyhow::Result<String> {
    let mut bytes = [0; 32];
    SysRng.try_fill_bytes(&mut bytes)?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let timestamp = now();
    tx.execute("DELETE FROM sessions WHERE expires_at < ?", [timestamp])?;
    tx.execute("INSERT INTO sessions(token_hash,created_at,expires_at,last_seen,user_agent) VALUES (?,?,?,?,?)", rusqlite::params![token_hash(&token),timestamp,timestamp+SESSION_TTL,timestamp,user_agent.chars().take(200).collect::<String>()])?;
    Ok(token)
}

fn expect_hash(tx: &rusqlite::Transaction<'_>, expected: Option<&str>) -> anyhow::Result<()> {
    let actual: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key=?",
            [PASSWORD_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if actual.as_deref() != expected {
        return Err(api_error(401, "The password changed. Sign in again."));
    }
    Ok(())
}

/// The verified hash is checked again in the session transaction. A login that
/// began before a password rotation cannot recreate a revoked session afterwards.
pub fn create_session_checked(
    db: &Database,
    user_agent: &str,
    expected: &str,
) -> anyhow::Result<String> {
    db.transaction(|tx| {
        expect_hash(tx, Some(expected))?;
        insert_session(tx, user_agent)
    })
}

pub fn rotate_password(
    db: &Database,
    expected: Option<&str>,
    password: Option<&str>,
    user_agent: &str,
) -> anyhow::Result<Option<String>> {
    let hashed = password.map(hash_password).transpose()?;
    db.transaction(|tx| {
        expect_hash(tx,expected)?;
        if let Some(value)=hashed.as_ref() {
            tx.execute("INSERT INTO settings(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",rusqlite::params![PASSWORD_KEY,value])?;
        } else {tx.execute("DELETE FROM settings WHERE key=?",[PASSWORD_KEY])?;}
        tx.execute("DELETE FROM sessions",[])?;
        hashed.as_ref().map(|_|insert_session(tx,user_agent)).transpose()
    })
}

pub fn session_valid(db: &Database, token: Option<&str>) -> anyhow::Result<bool> {
    let Some(token) = token.filter(|t| !t.is_empty() && t.len() <= 100) else {
        return Ok(false);
    };
    let digest = token_hash(token);
    let Some(row) = db.one(
        "SELECT expires_at,last_seen FROM sessions WHERE token_hash=?",
        &[json!(digest)],
    )?
    else {
        return Ok(false);
    };
    let timestamp = now();
    if row["expires_at"].as_i64().unwrap_or(0) < timestamp {
        return Ok(false);
    }
    if timestamp - row["last_seen"].as_i64().unwrap_or(0) > 300 {
        db.execute(
            "UPDATE sessions SET last_seen=?,expires_at=? WHERE token_hash=?",
            &[
                json!(timestamp),
                json!(timestamp + SESSION_TTL),
                json!(digest),
            ],
        )?;
    }
    Ok(true)
}

pub fn end_session(db: &Database, token: Option<&str>) -> anyhow::Result<()> {
    if let Some(token) = token.filter(|t| t.len() <= 100) {
        db.execute(
            "DELETE FROM sessions WHERE token_hash=?",
            &[json!(token_hash(token))],
        )?;
    }
    Ok(())
}

pub fn session_state(db: &Database, token: Option<&str>) -> anyhow::Result<Value> {
    let required = password_hash(db)?.is_some();
    Ok(
        json!({"authenticated": !required || session_valid(db,token)?, "password_set":required,"auth_required":required}),
    )
}

pub fn cookie_token(cookies: &str) -> Option<&str> {
    cookies.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == SESSION_COOKIE && value.len() <= 100).then_some(value)
    })
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; Max-Age={SESSION_TTL}; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}
pub fn clear_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

pub fn origin(value: &str) -> Option<(String, String, u16)> {
    let url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some((
        url.scheme().into(),
        url.host_str()?.to_lowercase(),
        url.port_or_known_default()?,
    ))
}

pub fn host_name(host: &str) -> Option<String> {
    origin(&format!("http://{host}")).map(|(_, host, _)| host)
}

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(String, String), (f64, Instant)>>,
}
/// The rate-limit identity of a client: its address, or for IPv6 its /64 network, since one
/// host or home network commonly holds a whole /64 and could otherwise rotate addresses.
fn client_bucket(client: &str) -> String {
    match client.parse::<IpAddr>().map(|ip| ip.to_canonical()) {
        Ok(IpAddr::V6(ip)) => {
            let s = ip.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => client.to_owned(),
    }
}

impl RateLimiter {
    pub fn allow(&self, client: &str, rule: &str, per_minute: u32) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        let key = (client_bucket(client), rule.to_owned());
        let instant = Instant::now();
        if buckets.len() >= 10_000 && !buckets.contains_key(&key) {
            buckets.retain(|_, (_, last)| last.elapsed().as_secs() < 120);
            if buckets.len() >= 10_000 {
                return false;
            }
        }
        let (tokens, last) = buckets
            .entry(key)
            .or_insert((f64::from(per_minute), instant));
        *tokens = (*tokens
            + instant.duration_since(*last).as_secs_f64() * f64::from(per_minute) / 60.0)
            .min(f64::from(per_minute));
        *last = instant;
        if *tokens < 1.0 {
            return false;
        }
        *tokens -= 1.0;
        true
    }
}

const RATE_RULES: &[(&str, &str, u32)] = &[
    ("POST", "/api/auth/login", 10),
    ("POST", "/api/auth/password", 10),
    ("POST", "/api/engine/", 90),
    ("POST", "/api/games/import", 20),
    ("POST", "/api/accounts", 30),
    ("GET", "/api/accounts/", 240),
];

/// The rate-limit bucket and per-minute budget for a request, if any. Practice answers are
/// cheap local writes made at play speed. They get their own budget instead of falling under
/// the `/api/accounts` rule, which is meant for adding and syncing accounts.
pub fn rate_rule(method: &str, path: &str) -> Option<(&'static str, u32)> {
    if method == "POST"
        && path.starts_with("/api/accounts/")
        && path.split('/').nth(4) == Some("practice")
    {
        return Some(("practice-answer", 600));
    }
    RATE_RULES
        .iter()
        .find(|(m, prefix, _)| *m == method && path.starts_with(prefix))
        .map(|&(_, prefix, rate)| (prefix, rate))
}

#[cfg(test)]
mod tests {
    #[test]
    fn ipv6_clients_share_a_bucket_per_64_network() {
        let limiter = RateLimiter::default();
        for host in 1..=3 {
            assert!(limiter.allow(&format!("2001:db8:1:2::{host}"), "login", 3));
        }
        // A fourth address in the same /64 is the same client.
        assert!(!limiter.allow("2001:db8:1:2:ffff::9", "login", 3));
        // Another /64 and IPv4 clients have their own budgets.
        assert!(limiter.allow("2001:db8:1:3::1", "login", 3));
        assert!(limiter.allow("192.0.2.1", "login", 3));
        assert!(limiter.allow("::ffff:192.0.2.2", "login", 3));
    }

    use super::*;
    #[test]
    fn practice_answers_do_not_share_the_account_budget() {
        assert_eq!(
            rate_rule("POST", "/api/accounts/1/practice/101/7"),
            Some(("practice-answer", 600))
        );
        assert_eq!(
            rate_rule("POST", "/api/accounts"),
            Some(("/api/accounts", 30))
        );
        assert_eq!(
            rate_rule("POST", "/api/accounts/1/sync"),
            Some(("/api/accounts", 30))
        );
        assert_eq!(
            rate_rule("GET", "/api/accounts/1/practice"),
            Some(("/api/accounts/", 240))
        );
        assert_eq!(rate_rule("GET", "/api/status"), None);
        let limiter = RateLimiter::default();
        // A brisk Practice session: well over the old 30 per minute.
        assert!((0..120).all(|_| limiter.allow("client", "practice-answer", 600)));
    }
    #[test]
    fn verifies_a_hash_made_independently_by_python_hashlib() {
        // hashlib.scrypt(b"correct horse battery", salt=bytes(range(16)), n=16384, r=8, p=1, dklen=32)
        let stored = "scrypt$16384$8$1$AAECAwQFBgcICQoLDA0ODw==$6W7GuoxjojaYIg83x/wEKSXUMMf+reyc6wSvA8q7ny8=";
        assert!(verify_password("correct horse battery", stored));
        assert!(!verify_password("correct horse batterz", stored));
        let fresh = hash_password("correct horse battery").unwrap();
        assert!(
            fresh.starts_with("scrypt$16384$8$1$")
                && verify_password("correct horse battery", &fresh)
        );
    }
    #[test]
    fn password_rotation_rejects_in_flight_old_credentials() -> anyhow::Result<()> {
        let db = Database::open(":memory:")?;
        set_password(&db, Some("original password"))?;
        let verified = password_hash(&db)?.unwrap();
        assert!(verify_password("original password", &verified));
        let token =
            rotate_password(&db, Some(&verified), Some("changed password"), "test")?.unwrap();
        assert!(create_session_checked(&db, "late login", &verified).is_err());
        assert!(rotate_password(&db, Some(&verified), Some("stale update"), "test").is_err());
        assert!(session_valid(&db, Some(&token))?);
        assert!(verify_password(
            "changed password",
            &password_hash(&db)?.unwrap()
        ));
        Ok(())
    }
    #[test]
    fn initial_password_setup_uses_compare_and_swap() -> anyhow::Result<()> {
        let db = Database::open(":memory:")?;
        rotate_password(&db, None, Some("first password"), "first")?;
        assert!(rotate_password(&db, None, Some("second password"), "second").is_err());
        Ok(())
    }
    #[test]
    fn origin_requires_exact_authority_and_scheme() {
        assert_eq!(
            origin("https://localhost"),
            origin("https://localhost:443/")
        );
        assert_ne!(origin("http://localhost"), origin("https://localhost"));
        for bad in [
            "null",
            "https://u@localhost",
            "https://localhost/x",
            "https://localhost?q=x",
            "https://localhost#x",
        ] {
            assert!(origin(bad).is_none());
        }
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("localhost.evil"));
    }
    #[test]
    fn token_bucket_does_not_reset_after_exhaustion() {
        let limiter = RateLimiter::default();
        for _ in 0..10 {
            assert!(limiter.allow("client", "login", 10));
        }
        assert!(!limiter.allow("client", "login", 10));
        assert!(limiter.allow("different", "login", 10));
    }
    #[test]
    fn password_and_session_compatibility() -> anyhow::Result<()> {
        let db = Database::open(":memory:")?;
        set_password(&db, Some("long password"))?;
        let hash = password_hash(&db)?.unwrap();
        assert!(verify_password("long password", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("x", "scrypt$999999999999$8$1$abc$abc"));
        let token = create_session(&db, "test")?;
        assert!(session_valid(&db, Some(&token))?);
        assert!(
            db.one(
                "SELECT token_hash FROM sessions WHERE token_hash=?",
                &[json!(token)]
            )?
            .is_none()
        );
        set_password(&db, Some("changed password"))?;
        assert!(!session_valid(&db, Some(&token))?);
        Ok(())
    }
}
