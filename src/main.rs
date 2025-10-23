use std::{str::FromStr, sync::Mutex, time::Duration};

use anyhow::Context as _;
use anyhow::anyhow;
use axum::Json;
use axum::extract::State;
use axum::{response::Response, routing::post};
use axum_session::{Session, SessionConfig, SessionLayer, SessionNullSessionStore};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use http::HeaderValue;
use http::StatusCode;
use log::info;
use log::warn;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as _;
use uuid::Uuid;

mod libvirt;

type SessionPool = axum_session::SessionNullPool;

static LIBVIRT: Mutex<Option<libvirt::Libvirt>> = Mutex::new(None);

#[derive(Deserialize)]
struct Config {
    static_web_path: String,
    listen_address: String,
    cookie_key: Option<String>,
    websocket_uri: String,
    libvirt: libvirt::Config,
    #[serde(
        deserialize_with = "duration_humantime",
        default = "default_cleanup_interval"
    )]
    cleanup_interval: Duration,
}

fn duration_humantime<'a, D>(de: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'a>,
{
    let string = String::deserialize(de)?;
    humantime::parse_duration(&string).map_err(|e| {
        D::Error::invalid_value(
            serde::de::Unexpected::Str(&format!("string {string:?}: {e:?}")),
            &"something that the humantime crate understands",
        )
    })
}

#[derive(Serialize)]
struct ConnectionDetails {
    ws_uri: String,
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

fn default_cleanup_interval() -> Duration {
    Duration::from_secs(60)
}

async fn cleanup_sessions(interval: Duration) {
    let mut interval = tokio::time::interval(interval);
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

    tokio::spawn(cleanup_sessions(config.cleanup_interval));

    let cookie_key = config.cookie_key.and_then(|ck| BASE64_STANDARD
        .decode(ck).ok())
        .and_then(|s| axum_session::Key::try_from(s.as_slice()).ok())
        .unwrap_or_else(|| {warn!("no session cookie key loaded, generating one");
            axum_session::Key::generate}());

    let session_config = SessionConfig::default().with_key(cookie_key);
    let session_store = SessionNullSessionStore::new(None, session_config)
        .await
        .context("creating session store")?;

    let app = axum::Router::new()
        .route("/keepalive", post(keepalive))
        .route("/connect", post(connect))
        .fallback_service(tower_http::services::ServeDir::new(config.static_web_path))
        .layer(SessionLayer::new(session_store))
        .with_state(config.websocket_uri);

    let listener = tokio::net::TcpListener::bind(config.listen_address)
        .await
        .unwrap();
    axum::serve(listener, app).await.context("serve")?;

    Ok(())
}
