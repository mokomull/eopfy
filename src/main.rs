use anyhow::Context as _;
use axum::{
    response::Response,
    routing::{get, post},
};
use axum_session::{Session, SessionConfig, SessionLayer, SessionNullSessionStore};
use http::HeaderValue;

type SessionPool = axum_session::SessionNullPool;

static INDEX_HTML: &str = include_str!("index.html");

async fn root(session: Session<SessionPool>) -> Response<String> {
    let data = INDEX_HTML
        .replace("SESSION_ID", &session.get_session_id())
        .replace("EXPIRE_TIME", "some time in the future idk");

    Response::builder()
        .header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
        )
        .body(data)
        .expect("building the result should succeed")
}

async fn keepalive() -> Response<String> {
    Response::builder()
        .status(http::StatusCode::SEE_OTHER)
        .header(http::header::LOCATION, HeaderValue::from_static("/"))
        .body("".into())
        .expect("building keepalive result should succeed")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let session_config = SessionConfig::default().with_key(axum_session::Key::generate());
    let session_store = SessionNullSessionStore::new(None, session_config)
        .await
        .context("creating session store")?;

    let app = axum::Router::new()
        .route("/", get(root))
        .route("/keepalive", post(keepalive))
        .layer(SessionLayer::new(session_store));

    let listener = tokio::net::TcpListener::bind(
        std::env::args()
            .nth(1)
            .expect("the first argument must be a listen address"),
    )
    .await
    .unwrap();
    axum::serve(listener, app).await.context("serve")?;

    Ok(())
}
