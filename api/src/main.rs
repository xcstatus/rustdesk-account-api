use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, parser::ValueSource};
use rustdesk_account_api::{
    admin, compat,
    credentials::CredentialCipher,
    limits::{LoginLimitConfig, LoginLimiter},
    oidc::{OidcConfig, OidcService},
    operations,
    store::Store,
    web_auth::{self, WebAuth},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(about = "RustDesk account API — single-instance account service")]
struct Args {
    #[arg(
        long,
        env = "RD_DATABASE_URL",
        default_value = "sqlite://data/accounts.sqlite",
        global = true
    )]
    database_url: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize the first local administrator once. Password: RD_BOOTSTRAP_PASSWORD.
    BootstrapAdmin {
        username: String,
    },
    /// Create a test/local user. Password comes from RD_BOOTSTRAP_PASSWORD, never argv.
    CreateLocal {
        username: String,
        #[arg(long, default_value = "member")]
        role: String,
    },
    DisableUser {
        user_id: String,
    },
    /// Apply forward-only database migrations and exit.
    Migrate,
    /// Validate the configured key ring and every stored book credential, then exit.
    CheckCredentials,
    /// Check schema, database writability and the Web build, then exit.
    ReadyCheck,
    /// Delete a bounded batch of audit records older than this retention period.
    PruneAudit {
        #[arg(long, default_value_t = 90)]
        retention_days: u32,
        #[arg(long, default_value_t = 1000)]
        batch: u32,
    },
    Serve {
        #[arg(long, env = "RD_LISTEN", default_value = "127.0.0.1:21114")]
        listen: SocketAddr,
    },
}
fn optional_setting(name: &str) -> Result<Option<String>> {
    let direct = std::env::var(name).ok();
    let file = std::env::var(format!("{name}_FILE")).ok();
    ensure!(
        direct.is_none() || file.is_none(),
        "set only {name} or {name}_FILE"
    );
    let value = if let Some(path) = file {
        let mut value =
            std::fs::read_to_string(path).with_context(|| format!("cannot read {name}_FILE"))?;
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        Some(value)
    } else {
        direct
    };
    Ok(value.filter(|v| !v.is_empty()))
}
fn setting(name: &str) -> Result<String> {
    optional_setting(name)?.with_context(|| format!("missing {name}"))
}
fn number<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match std::env::var(name) {
        Ok(v) => v.parse().map_err(|_| anyhow::anyhow!("invalid {name}")),
        Err(_) => Ok(default),
    }
}
fn web_directory() -> PathBuf {
    std::env::var("RD_WEB_DIR")
        .unwrap_or_else(|_| "web/dist".into())
        .into()
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
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
    tracing::info!("shutdown requested");
}
fn values(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("rustdesk_account_api=info")
        .init();
    let matches = Args::command().get_matches();
    let mut args = Args::from_arg_matches(&matches)?;
    if std::env::var_os("RD_DATABASE_URL_FILE").is_some() {
        ensure!(
            matches.value_source("database_url") != Some(ValueSource::CommandLine),
            "set only --database-url or RD_DATABASE_URL_FILE"
        );
        args.database_url = setting("RD_DATABASE_URL")?;
    }
    std::fs::create_dir_all("data")?;
    let cipher = std::env::var_os("RD_BOOK_KEYS_FILE")
        .map(|path| CredentialCipher::from_file(&PathBuf::from(path)).map(Arc::new))
        .transpose()?;
    let store = Store::connect(&args.database_url)
        .await?
        .configure_credential_cipher(cipher)
        .await?;
    match args.command {
        Command::BootstrapAdmin { username } => {
            let user = store
                .bootstrap_admin(&username, &setting("RD_BOOTSTRAP_PASSWORD")?)
                .await?;
            println!("initialized administrator {} ({})", user.username, user.id);
        }
        Command::CreateLocal { username, role } => {
            let password = setting("RD_BOOTSTRAP_PASSWORD")?;
            ensure!(
                password.len() >= 12,
                "local password must have at least 12 bytes"
            );
            let user = store.create_local(&username, &password, &role).await?;
            println!("created user {} ({})", user.username, user.id);
        }
        Command::DisableUser { user_id } => store.disable_user(&user_id).await?,
        Command::Migrate => println!("database migrations complete"),
        Command::CheckCredentials => println!("stored credential validation complete"),
        Command::ReadyCheck => operations::ready(&store, &web_directory()).await?,
        Command::PruneAudit {
            retention_days,
            batch,
        } => {
            ensure!(
                (1..=36500).contains(&retention_days),
                "retention days must be 1..36500"
            );
            let cutoff = chrono::Utc::now().timestamp() - i64::from(retention_days) * 86400;
            let deleted = store.prune_audit(cutoff, batch).await?;
            println!("pruned {deleted} audit events");
        }
        Command::Serve { listen } => {
            ensure!(
                listen.ip().is_loopback()
                    || std::env::var("RD_ALLOW_NON_LOOPBACK").as_deref() == Ok("true"),
                "non-loopback listen requires RD_ALLOW_NON_LOOPBACK=true and an HTTPS reverse proxy"
            );
            let ttl = std::env::var("RD_SESSION_TTL_SECONDS")
                .unwrap_or_else(|_| "2592000".into())
                .parse::<i64>()?;
            ensure!(ttl > 0, "session lifetime must be positive");
            let oidc = if let Ok(issuer) = std::env::var("RD_OIDC_ISSUER") {
                let unmatched =
                    std::env::var("RD_OIDC_UNMATCHED").unwrap_or_else(|_| "deny".into());
                ensure!(
                    matches!(unmatched.as_str(), "deny" | "member"),
                    "RD_OIDC_UNMATCHED must be deny or member"
                );
                Some(Arc::new(
                    OidcService::new(
                        OidcConfig {
                            issuer,
                            client_id: setting("RD_OIDC_CLIENT_ID")?,
                            client_secret: optional_setting("RD_OIDC_CLIENT_SECRET")?,
                            redirect_uri: setting("RD_OIDC_REDIRECT_URI")?,
                            claim_path: std::env::var("RD_OIDC_CLAIM_PATH")
                                .unwrap_or_else(|_| "groups".into())
                                .split('.')
                                .map(str::to_owned)
                                .collect(),
                            admin_values: values("RD_OIDC_ADMIN_VALUES"),
                            member_values: values("RD_OIDC_MEMBER_VALUES"),
                            unmatched_member: unmatched == "member",
                            allow_http_loopback: std::env::var("RD_ALLOW_HTTP_LOOPBACK").as_deref()
                                == Ok("true"),
                            session_ttl_seconds: ttl,
                        },
                        store.clone(),
                    )
                    .await?,
                ))
            } else {
                store.sync_policy_version("oidc-disabled-v1").await?;
                None
            };
            let origin =
                std::env::var("RD_WEB_ORIGIN").unwrap_or_else(|_| format!("http://{listen}"));
            let web_ttl = std::env::var("RD_WEB_SESSION_TTL_SECONDS")
                .unwrap_or_else(|_| "28800".into())
                .parse::<i64>()?;
            let limiter = LoginLimiter::new(LoginLimitConfig {
                window_seconds: number("RD_LOGIN_WINDOW_SECONDS", 60)?,
                per_account: number("RD_LOGIN_PER_ACCOUNT", 10)?,
                global: number("RD_LOGIN_GLOBAL", 300)?,
                max_keys: number("RD_LOGIN_MAX_KEYS", 1024)?,
            })?;
            let auth =
                WebAuth::new(store.clone(), web_ttl, &origin)?.with_login_limiter(limiter.clone());
            if !listen.ip().is_loopback() {
                ensure!(
                    auth.secure_cookies,
                    "external listener requires an HTTPS Web origin"
                );
            }
            let web_dir = web_directory();
            operations::ready(&store, &web_dir)
                .await
                .context("startup readiness failed")?;
            let retention_days: u32 = number("RD_AUDIT_RETENTION_DAYS", 90)?;
            ensure!(
                retention_days <= 36500,
                "audit retention must be 0..36500 days"
            );
            let maintenance = if retention_days > 0 {
                let store = store.clone();
                Some(tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                    loop {
                        interval.tick().await;
                        let cutoff =
                            chrono::Utc::now().timestamp() - i64::from(retention_days) * 86400;
                        // Bound maintenance per hour to avoid monopolizing the writer.
                        for _ in 0..10 {
                            match store.prune_audit(cutoff, 1000).await {
                                Ok(1000) => tokio::task::yield_now().await,
                                Ok(_) => break,
                                Err(_) => {
                                    tracing::error!("audit retention cleanup failed");
                                    break;
                                }
                            }
                        }
                    }
                }))
            } else {
                None
            };
            let app = compat::router_with_limiter(store.clone(), ttl, oidc.clone(), limiter)
                .merge(web_auth::router(auth.clone(), oidc))
                .merge(admin::router(auth))
                .merge(rustdesk_account_api::web_files::router(
                    web_dir.to_string_lossy().into_owned(),
                ))
                .merge(operations::router(store, web_dir))
                .layer(axum::middleware::map_response(web_auth::security_headers))
                .layer(axum::middleware::from_fn(operations::log_request));
            let listener = tokio::net::TcpListener::bind(listen).await?;
            tracing::info!(%listen,"Account API ready");
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await;
            if let Some(maintenance) = maintenance {
                maintenance.abort();
            }
            result?;
        }
    }
    Ok(())
}
