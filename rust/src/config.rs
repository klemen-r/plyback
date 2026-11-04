use std::path::PathBuf;

pub const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub data: PathBuf,
    pub frontend: PathBuf,
    pub host: String,
    pub port: u16,
    pub allowed_hosts: Vec<String>,
    pub secure_cookies: bool,
    pub allow_unauthenticated_remote: bool,
    pub background: bool,
}

pub fn env(name: &str, legacy: Option<&str>) -> Option<String> {
    std::env::var(format!("PLYBACK_{name}"))
        .ok()
        .or_else(|| legacy.and_then(|key| std::env::var(key).ok()))
        .filter(|value| !value.is_empty())
}

pub fn flag(name: &str) -> bool {
    env(name, None)
        .map(|s| {
            matches!(
                s.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        // Distribution launchers set these explicitly. A source invocation uses the
        // repository, independent of the directory from which cargo was invoked.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        Ok(Self {
            data: PathBuf::from(
                env("DATA", Some("CHESS_INSIGHT_DATA"))
                    .unwrap_or_else(|| root.join("data").to_string_lossy().into()),
            ),
            frontend: PathBuf::from(
                env("FRONTEND", Some("CHESS_INSIGHT_FRONTEND"))
                    .unwrap_or_else(|| root.join("frontend/dist").to_string_lossy().into()),
            ),
            host: env("HOST", Some("HOST")).unwrap_or_else(|| "127.0.0.1".into()),
            port: env("PORT", Some("PORT"))
                .unwrap_or_else(|| "8000".into())
                .parse()?,
            allowed_hosts: env("ALLOWED_HOSTS", None)
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            secure_cookies: flag("SECURE_COOKIES"),
            allow_unauthenticated_remote: flag("ALLOW_UNAUTHENTICATED_REMOTE"),
            background: !flag("NO_BACKGROUND"),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data.join("plyback.db")
    }
}
