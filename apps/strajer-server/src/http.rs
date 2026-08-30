use axum::Json;
use axum::extract::State;
use axum::routing::get;
use axum::{Router, http::StatusCode};
use serde::Serialize;
use strajer_protocol::LobbyCatalog;
use tower_http::trace::TraceLayer;

use crate::AppState;

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route("/v1/lobbies", get(lobbies))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "strajer-server",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn readiness(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let status = if state.catalog().validate().is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            status: if status == StatusCode::OK {
                "ready"
            } else {
                "not_ready"
            },
            service: "strajer-server",
            version: env!("CARGO_PKG_VERSION"),
        }),
    )
}

async fn lobbies(State(state): State<AppState>) -> Json<LobbyCatalog> {
    Json(state.catalog().clone())
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use strajer_protocol::{CATALOG_SCHEMA_VERSION, LobbyCatalog};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn serves_a_valid_synthetic_catalog() {
        let state = AppState::synthetic_at(2_000, 2).expect("state should be valid");
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/v1/lobbies")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1_024)
            .await
            .expect("body should be readable");
        let catalog: LobbyCatalog =
            serde_json::from_slice(&body).expect("catalog should deserialize");

        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(catalog.generated_at_unix_ms, 2_000);
        assert_eq!(catalog.lobbies.len(), 1);
        assert_eq!(catalog.lobbies[0].name, "Strajer Test #1");
        assert_eq!(catalog.validate(), Ok(()));
    }

    #[tokio::test]
    async fn reports_readiness() {
        let state = AppState::synthetic_at(2_000, 2).expect("state should be valid");
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
