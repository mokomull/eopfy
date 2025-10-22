use std::{borrow::Cow, str::FromStr, sync::Mutex, time::Duration};

use anyhow::Context as _;
use anyhow::anyhow;
use axum::Json;
use axum::extract::State;
use axum::{
    response::Response,
    routing::{get, post},
};
use axum_session::{Session, SessionConfig, SessionLayer, SessionNullSessionStore};
use http::HeaderValue;
use http::StatusCode;
use log::info;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

mod libvirt;

type SessionPool = axum_session::SessionNullPool;

static INDEX_HTML: &str = include_str!("index.html");

const SESSION_DURATION: Duration = Duration::from_secs(30);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

static LIBVIRT: Mutex<Option<libvirt::Libvirt>> = Mutex::new(None);

#[derive(Deserialize)]
struct Config {
    listen_address: String,
    websocket_uri: String,
    libvirt: libvirt::Config,
}

#[derive(Serialize)]
struct ConnectionDetails {
    ws_uri: String,
}

async fn root(session: Session<SessionPool>) -> Response<String> {
    let expire_time = {
        let mut libvirt = LIBVIRT.lock().unwrap();
        let libvirt = libvirt.as_mut().unwrap();
        libvirt
            .get_expiration_for(
                Uuid::from_str(&session.get_session_id()).expect("session IDs are UUIDs"),
            )
            .expect("get_expiration_for") // TODO: actually return a 500 instead of crashing
    }
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

async fn connect(
    uri: State<String>,
    session: Session<SessionPool>,
) -> axum::response::Result<Json<ConnectionDetails>> {
    let token = {
        let mut libvirt = LIBVIRT.lock().unwrap();
        let libvirt = libvirt.as_mut().unwrap();
        libvirt
            .create_or_keepalive(
                Uuid::from_str(&session.get_session_id()).expect("session IDs are UUIDs"),
            )
            .map_err(|e| {
                log::error!("create_or_keepalive failed: {e:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "could not connect to VM")
            })?
    };

    // TODO: actually handle query strings intelligently but the http crate's Uri builder pattern
    // doesn't make it easy to just add a query parameter
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("token", &token)
        .finish();

    Ok(Json(ConnectionDetails {
        ws_uri: format!("{}?{}", uri.0, query),
    }))
}

async fn keepalive(session: Session<SessionPool>) -> Response<String> {
    let mut libvirt = LIBVIRT.lock().unwrap();
    let libvirt = libvirt.as_mut().unwrap();
    libvirt
        .create_or_keepalive(
            Uuid::from_str(&session.get_session_id()).expect("session IDs are UUIDs"),
        )
        .expect("create_or_keepalive"); // TODO: actually return a 500 instead of crashing
    Response::builder()
        .status(http::StatusCode::SEE_OTHER)
        .header(http::header::LOCATION, HeaderValue::from_static("/"))
        .body("".into())
        .expect("building keepalive result should succeed")
}

async fn cleanup_sessions() {
    let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        info!("cleaning up sessions");

        let mut libvirt = LIBVIRT.lock().unwrap();
        let libvirt = libvirt.as_mut().unwrap();
        libvirt.expire().expect("expiration process should succeed");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

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

    let libvirt = libvirt::Libvirt::connect(config.libvirt).context("initializing libvirt")?;
    *LIBVIRT.lock().unwrap() = Some(libvirt);

    tokio::spawn(cleanup_sessions());

    let session_config = SessionConfig::default().with_key(axum_session::Key::generate());
    let session_store = SessionNullSessionStore::new(None, session_config)
        .await
        .context("creating session store")?;

    let app = axum::Router::new()
        .route("/", get(root))
        .route("/keepalive", post(keepalive))
        .route("/connect", post(connect))
        .layer(SessionLayer::new(session_store))
        .with_state(config.websocket_uri);

    let listener = tokio::net::TcpListener::bind(config.listen_address)
        .await
        .unwrap();
    axum::serve(listener, app).await.context("serve")?;

    Ok(())
}
