use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

pub const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub data: PathBuf,
    pub frontend: PathBuf,
    pub host: String,
    pub port: u16,
    pub allowed_hosts: Vec<String>,
    pub trusted_proxies: Vec<IpAddr>,
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
        // Distribution launchers and the container set these explicitly; otherwise
        // the paths follow the executable, not the checkout that happened to build it.
        let (data, frontend) = resolve_paths(
            std::env::current_exe().ok().as_deref(),
            &build_root(),
            env("DATA", Some("CHESS_INSIGHT_DATA")),
            env("FRONTEND", Some("CHESS_INSIGHT_FRONTEND")),
        );
        Ok(Self {
            data,
            frontend,
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
            trusted_proxies: parse_trusted_proxies(
                &env("TRUSTED_PROXIES", None).unwrap_or_default(),
            )?,
            allow_unauthenticated_remote: flag("ALLOW_UNAUTHENTICATED_REMOTE"),
            background: !flag("NO_BACKGROUND"),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data.join("plyback.db")
    }

    pub fn requires_password(&self) -> bool {
        !crate::security::is_loopback_host(&self.host) && !self.allow_unauthenticated_remote
    }
}

fn parse_trusted_proxies(value: &str) -> anyhow::Result<Vec<IpAddr>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<IpAddr>()
                .map(|ip| ip.to_canonical())
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Invalid IP address in PLYBACK_TRUSTED_PROXIES: {s}. \
                         Use exact IPs, not names or CIDR ranges."
                    )
                })
        })
        .collect()
}

/// The checkout that compiled this binary. Only a last resort: the executable may
/// since have been copied elsewhere or the checkout moved.
fn build_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_owned()
}

/// Whether `root` looks like a Plyback installation: a built interface or a database.
fn is_install_root(root: &Path) -> bool {
    root.join("frontend/dist/index.html").is_file()
        || root.join("data/plyback.db").is_file()
        || root.join("data/chess-insight.db").is_file()
}

/// The directory that holds `data/` and `frontend/dist/` when neither is configured.
///
/// A source checkout runs `<root>/rust/target/release/plyback`, so the executable's
/// directory and its next three ancestors are searched for an installation; this also
/// covers an executable copied next to `frontend/`. Then the building checkout, if it
/// is still an installation. Otherwise the root implied by the checkout layout, or
/// the building checkout when the executable is not inside one.
pub fn install_root(exe: Option<&Path>, build_root: &Path) -> PathBuf {
    let ancestors: Vec<&Path> = exe
        .and_then(Path::parent)
        .map(|dir| dir.ancestors().take(4).collect())
        .unwrap_or_default();
    if let Some(root) = ancestors.iter().find(|root| is_install_root(root)) {
        return root.to_path_buf();
    }
    if is_install_root(build_root) {
        return build_root.to_owned();
    }
    // <root>/rust/target/<profile>/plyback
    match ancestors.as_slice() {
        [_, target, rust, root, ..]
            if target.file_name().is_some_and(|n| n == "target")
                && rust.file_name().is_some_and(|n| n == "rust") =>
        {
            root.to_path_buf()
        }
        _ => build_root.to_owned(),
    }
}

/// Explicit settings win; otherwise both directories sit under [`install_root`].
pub fn resolve_paths(
    exe: Option<&Path>,
    build_root: &Path,
    data: Option<String>,
    frontend: Option<String>,
) -> (PathBuf, PathBuf) {
    let root = || install_root(exe, build_root);
    (
        data.map(PathBuf::from)
            .unwrap_or_else(|| root().join("data")),
        frontend
            .map(PathBuf::from)
            .unwrap_or_else(|| root().join("frontend").join("dist")),
    )
}

/// The data directory a server would use, for code that runs without a [`Config`].
pub fn default_data_dir() -> PathBuf {
    resolve_paths(
        std::env::current_exe().ok().as_deref(),
        &build_root(),
        env("DATA", Some("CHESS_INSIGHT_DATA")),
        None,
    )
    .0
}

/// Refuse to serve when the interface is missing: every page would be a 404.
pub fn check_frontend(frontend: &Path) -> anyhow::Result<()> {
    if frontend.join("index.html").is_file() {
        return Ok(());
    }
    anyhow::bail!(
        "The web interface is not built: {} has no index.html. Run the setup script \
         (or `npm run build` in frontend/), or set --frontend / PLYBACK_FRONTEND to a built frontend/dist.",
        frontend.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_proxies_require_explicit_ips() {
        assert!(parse_trusted_proxies("").unwrap().is_empty());
        assert_eq!(
            parse_trusted_proxies("127.0.0.1, ::1, , ::ffff:192.0.2.1").unwrap(),
            ["127.0.0.1", "::1", "192.0.2.1"].map(|s| s.parse::<IpAddr>().unwrap())
        );
        for invalid in [
            "*",
            "localhost",
            "127.0.0.0/8",
            "127.0.0.1:8000",
            "127.0.0.1,garbage",
        ] {
            assert!(
                parse_trusted_proxies(invalid)
                    .unwrap_err()
                    .to_string()
                    .contains("PLYBACK_TRUSTED_PROXIES")
            );
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn exe(root: &Path) -> PathBuf {
        root.join("rust/target/release/plyback.exe")
    }

    #[test]
    fn source_checkout_follows_the_executable_after_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let (moved, old) = (dir.path().join("moved"), dir.path().join("old"));
        touch(&moved.join("frontend/dist/index.html"));
        touch(&old.join("frontend/dist/index.html"));
        let (data, frontend) = resolve_paths(Some(&exe(&moved)), &old, None, None);
        assert_eq!(data, moved.join("data"));
        assert_eq!(frontend, moved.join("frontend").join("dist"));
    }

    #[test]
    fn an_existing_database_identifies_the_root_without_a_built_interface() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("checkout");
        touch(&root.join("data/plyback.db"));
        let other = dir.path().join("other");
        touch(&other.join("frontend/dist/index.html"));
        assert_eq!(install_root(Some(&exe(&root)), &other), root);
    }

    #[test]
    fn copied_executable_next_to_the_interface_and_layout_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join("beside");
        touch(&beside.join("frontend/dist/index.html"));
        let build = dir.path().join("build");
        assert_eq!(install_root(Some(&beside.join("plyback")), &build), beside);
        // Nothing built anywhere: the checkout layout around the executable decides.
        let fresh = dir.path().join("fresh");
        assert_eq!(install_root(Some(&exe(&fresh)), &build), fresh);
        // A lone copy outside any checkout falls back to the building checkout.
        let lone = dir.path().join("bin/plyback");
        assert_eq!(install_root(Some(&lone), &build), build);
        assert_eq!(install_root(None, &build), build);
        // An installation around the executable beats the building checkout.
        touch(&build.join("frontend/dist/index.html"));
        assert_eq!(install_root(Some(&beside.join("plyback")), &build), beside);
    }

    #[test]
    fn explicit_settings_win() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("checkout");
        touch(&root.join("frontend/dist/index.html"));
        let (data, frontend) = resolve_paths(
            Some(&exe(&root)),
            &root,
            Some("/srv/data".into()),
            Some("/srv/ui".into()),
        );
        assert_eq!(data, PathBuf::from("/srv/data"));
        assert_eq!(frontend, PathBuf::from("/srv/ui"));
    }

    #[test]
    fn missing_interface_is_reported_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let error = check_frontend(dir.path()).unwrap_err().to_string();
        assert!(error.contains(&dir.path().display().to_string()));
        assert!(error.contains("PLYBACK_FRONTEND"));
        touch(&dir.path().join("index.html"));
        check_frontend(dir.path()).unwrap();
    }
}
