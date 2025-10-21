use std::{
    borrow::Cow,
    time::{Duration, SystemTime},
};

use anyhow::Context as _;
use anyhow::anyhow;
use axum::{
    response::Response,
    routing::{get, post},
};
use axum_session::{Session, SessionConfig, SessionLayer, SessionNullSessionStore};
use http::HeaderValue;
use serde::Deserialize;

mod libvirt;

type SessionPool = axum_session::SessionNullPool;

static INDEX_HTML: &str = include_str!("index.html");

static SESSION_EXPIRATION_KEY: &str = "session_expiration";
const SESSION_DURATION: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Config {
    listen_address: String,
    libvirt: libvirt::Config,
}

async fn root(session: Session<SessionPool>) -> Response<String> {
    let expire_time = session
        .get::<SystemTime>(SESSION_EXPIRATION_KEY)
        .map(|t| Cow::Owned(humantime::format_rfc3339(t).to_string()))
        .unwrap_or(Cow::Borrowed("some time in the future idk"));
    let data = INDEX_HTML
        .replace("SESSION_ID", &session.get_session_id())
        .replace("EXPIRE_TIME", &expire_time);

    Response::builder()
        .header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
        )
        .body(data)
        .expect("building the result should succeed")
}

async fn keepalive(session: Session<SessionPool>) -> Response<String> {
    session.set(SESSION_EXPIRATION_KEY, SystemTime::now() + SESSION_DURATION);
    Response::builder()
        .status(http::StatusCode::SEE_OTHER)
        .header(http::header::LOCATION, HeaderValue::from_static("/"))
        .body("".into())
        .expect("building keepalive result should succeed")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = std::env::args().nth(1).ok_or(anyhow!(
        "you must provide the config file on the command line"
    ))?;
    let config: Config = toml::from_str(
        str::from_utf8(
            &std::fs::read(&config_path)
                .with_context(|| format!("could not read config file {:?}", config_path))?,
        )
        .context("config file is not UTF8")?,
    )
    .context("could not parse TOML")?;

    let session_config = SessionConfig::default().with_key(axum_session::Key::generate());
    let session_store = SessionNullSessionStore::new(None, session_config)
        .await
        .context("creating session store")?;

    let app = axum::Router::new()
        .route("/", get(root))
        .route("/keepalive", post(keepalive))
        .layer(SessionLayer::new(session_store));

    let listener = tokio::net::TcpListener::bind(config.listen_address)
        .await
        .unwrap();
    axum::serve(listener, app).await.context("serve")?;

    Ok(())
}
