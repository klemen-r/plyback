use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use plyback::{
    config::{self, Config},
    db::Database,
    http::App,
    security,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    #[arg(long, global = true)]
    host: Option<String>,
    #[arg(long, global = true)]
    port: Option<u16>,
    #[arg(long, global = true)]
    frontend: Option<PathBuf>,
    #[arg(long, global = true)]
    no_background: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the API and serve the built web interface.
    Serve,
    /// Set or change the access password without exposing it in process arguments.
    SetPassword,
    /// Remove the access password for local use.
    ClearPassword,
    /// Save a consistent SQLite snapshot, refusing to overwrite an existing file.
    Backup { destination: PathBuf },
    /// Print settings, or update them with a JSON object.
    Settings { patch: Option<String> },
    /// Check the local server (also used by the container health check).
    Healthcheck,
}

fn backup(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.is_file() {
        bail!("Source database does not exist: {}", source.display());
    }
    if destination.exists() {
        bail!("Destination already exists: {}", destination.display());
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let reader = rusqlite::Connection::open_with_flags(
            source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut writer = rusqlite::Connection::open(temporary.path())?;
        rusqlite::backup::Backup::new(&reader, &mut writer)?.run_to_completion(
            256,
            std::time::Duration::from_millis(10),
            None,
        )?;
        let check: String = writer.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            bail!("Database snapshot failed its integrity check");
        }
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .context("Could not publish backup without overwriting destination")?;
    Ok(())
}

fn adopt_legacy(config: &Config) -> anyhow::Result<()> {
    let legacy = config.data.join("chess-insight.db");
    if !config.db_path().exists() && legacy.is_file() {
        backup(&legacy, &config.db_path())?;
        tracing::info!("Copied legacy database; original retained for rollback");
    }
    Ok(())
}

// A bounded local log keeps the source distribution's existing logs command useful.
#[derive(Clone)]
struct Logs(Arc<Mutex<LogFile>>);
struct LogFile {
    path: PathBuf,
    file: std::fs::File,
    size: u64,
}
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(bytes);
        let mut log = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if log.size + bytes.len() as u64 > 5_000_000 {
            // Close before rotating so this also works on Windows.
            let temporary = log.path.with_extension("rotating");
            let replacement = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            let old = std::mem::replace(&mut log.file, replacement);
            drop(old);
            let _ = std::fs::remove_file(log.path.with_extension("log.3"));
            for index in (1..3).rev() {
                let _ = std::fs::rename(
                    log.path.with_extension(format!("log.{index}")),
                    log.path.with_extension(format!("log.{}", index + 1)),
                );
            }
            std::fs::rename(&log.path, log.path.with_extension("log.1"))?;
            // Reopen after closing the staging handle, then publish the new path.
            let placeholder = std::fs::OpenOptions::new()
                .append(true)
                .open(log.path.with_extension("log.1"))?;
            let old = std::mem::replace(&mut log.file, placeholder);
            drop(old);
            std::fs::rename(&temporary, &log.path)?;
            log.file = std::fs::OpenOptions::new().append(true).open(&log.path)?;
            log.size = 0;
        }
        log.file.write_all(bytes)?;
        log.size += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .file
            .flush()
    }
}

fn logging(config: &Config) {
    let filter = tracing_subscriber::EnvFilter::try_new(
        config::env("LOG_LEVEL", Some("LOG_LEVEL")).unwrap_or_else(|| "info".into()),
    )
    .unwrap_or_else(|_| "info".into());
    let path = config.data.join("logs/plyback.log");
    let writer = (|| -> std::io::Result<Logs> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let size = file.metadata()?.len();
        Ok(Logs(Arc::new(Mutex::new(LogFile { path, file, size }))))
    })();
    if let Ok(writer) = writer {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = Config::from_env()?;
    if let Some(data) = cli.data {
        config.data = data;
    }
    if let Some(host) = cli.host {
        config.host = host;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(frontend) = cli.frontend {
        config.frontend = frontend;
    }
    if cli.no_background {
        config.background = false;
    }
    // The engine looks for data/engine/stockfish* in the same data directory.
    plyback::engine::set_data_dir(config.data.clone());
    if matches!(cli.command, Some(Command::Healthcheck)) {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(4))
            .build()?
            .get(format!("http://127.0.0.1:{}/api/health", config.port))
            .send()?
            .error_for_status()?;
        return Ok(());
    }
    if let Some(Command::Backup { destination }) = cli.command {
        let source = if config.db_path().exists() {
            config.db_path()
        } else {
            config.data.join("chess-insight.db")
        };
        backup(&source, &destination)?;
        println!("Database snapshot saved to {}", destination.display());
        return Ok(());
    }
    // Reserve the address before opening or migrating data. A second launch on
    // an occupied port must not reset the active instance's running job claims.
    let listener = if matches!(cli.command, None | Some(Command::Serve)) {
        config::check_frontend(&config.frontend)?;
        let listener = std::net::TcpListener::bind((config.host.as_str(), config.port))?;
        listener.set_nonblocking(true)?;
        Some(listener)
    } else {
        None
    };
    logging(&config);
    adopt_legacy(&config)?;
    let db = Database::open(config.db_path())?;
    match cli.command {
        Some(Command::SetPassword) => {
            // Name the database, so a wrong data directory is noticed at once.
            println!("Database: {}", config.db_path().display());
            let first = rpassword::prompt_password("New password: ")?;
            if first != rpassword::prompt_password("Repeat password: ")? {
                bail!("Passwords do not match.");
            }
            security::validate_password(&first)?;
            security::set_password(&db, Some(&first))?;
            println!("Password set. Existing sessions were signed out.");
            return Ok(());
        }
        Some(Command::ClearPassword) => {
            if !security::is_loopback_host(&config.host) {
                bail!(
                    "Stop network access and select a loopback host before removing the password."
                );
            }
            security::set_password(&db, None)?;
            println!("Password removed.");
            return Ok(());
        }
        Some(Command::Settings { patch }) => {
            let settings = if let Some(patch) = patch {
                plyback::services::save_settings(&db, &serde_json::from_str(&patch)?)?
            } else {
                plyback::services::get_settings(&db)?
            };
            println!("{}", serde_json::to_string_pretty(&settings)?);
            return Ok(());
        }
        _ => {}
    }
    if let Some(password) = config::env("PASSWORD", None)
        && security::password_hash(&db)?.is_none()
    {
        security::validate_password(&password)?;
        security::set_password(&db, Some(&password))?;
    }
    if config.requires_password() && security::password_hash(&db)?.is_none() {
        bail!(
            "Refusing network access without a password. Use `plyback set-password` or PLYBACK_PASSWORD."
        );
    }
    drop(db);
    // Construct blocking clients before entering Tokio; their private runtimes
    // must not be created/dropped inside an async task.
    let app = App::open(config.clone())?;
    app.start();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(32)
        .build()?;
    let result=runtime.block_on(async {
        let listener=tokio::net::TcpListener::from_std(listener.expect("serve reserved a listener"))?;
        tracing::info!(version=plyback::APP_VERSION,address=%listener.local_addr()?,"Plyback started");
        let shutdown=app.clone();
        axum::serve(listener,plyback::http::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
            .with_graceful_shutdown(async move {shutdown_signal().await;shutdown.request_stop();}).await?;
        Ok::<(),anyhow::Error>(())
    });
    app.stop();
    drop(app);
    result
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {_ = tokio::signal::ctrl_c()=>{}, _ = terminate.recv()=>{}}
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_database_is_adopted_by_copy() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut config = Config::from_env()?;
        config.data = dir.path().to_path_buf();
        let source = dir.path().join("chess-insight.db");
        let writer = rusqlite::Connection::open(&source)?;
        writer.execute_batch(
            "CREATE TABLE evidence(value INTEGER); INSERT INTO evidence VALUES(7)",
        )?;
        drop(writer);
        let original = std::fs::read(&source)?;
        adopt_legacy(&config)?;
        assert_eq!(std::fs::read(&source)?, original);
        let reader = rusqlite::Connection::open(config.db_path())?;
        assert_eq!(
            reader.query_row("SELECT value FROM evidence", [], |r| r.get::<_, i64>(0))?,
            7
        );
        drop(reader);
        let adopted = std::fs::read(config.db_path())?;
        adopt_legacy(&config)?;
        assert_eq!(std::fs::read(config.db_path())?, adopted);
        assert_eq!(std::fs::read(source)?, original);
        Ok(())
    }
    #[test]
    fn backup_reads_committed_wal_and_never_overwrites() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("source.db");
        let target = dir.path().join("backup.db");
        let conn = rusqlite::Connection::open(&source)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE preserved(value TEXT); INSERT INTO preserved VALUES('retained');")?;
        backup(&source, &target)?;
        assert_eq!(
            rusqlite::Connection::open(&target)?.query_row(
                "SELECT value FROM preserved",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "retained"
        );
        assert!(backup(&source, &target).is_err());
        Ok(())
    }
}
