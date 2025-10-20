use anyhow::Context as _;
use axum::{response::Response, routing::get};
use http::HeaderValue;

static INDEX_HTML: &str = include_str!("index.html");

async fn root() -> Response<String> {
    Response::builder()
        .header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
        )
        .body(INDEX_HTML.to_owned())
        .expect("building the result should succeed")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = axum::Router::new().route("/", get(root));
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
