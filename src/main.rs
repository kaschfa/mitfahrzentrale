use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};

#[derive(Clone)]
struct AppState {
    pool: PgPool,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct Entry {
    id: i32,
    titel: String,
    nachricht: String,
    typ: String,
    sitzplaetze: i32,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct User {
    id: i32,
    nachname: String,
    email: String,
    status: String,
    token: String,
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect("postgres://app:app@localhost:5432/app_db")
        .await?;

    let app = Router::new()
        .route("/", get(list_entries))
        .route("/users", get(list_users))
        .with_state(AppState { pool });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

async fn get_all_entries(pool: &sqlx::PgPool) -> Result<Vec<Entry>, sqlx::Error> {
    sqlx::query_as::<_, Entry>(
        r#"
        SELECT id, titel, nachricht, typ, sitzplaetze
        FROM eintrag
        "#,
    )
    .fetch_all(pool)
    .await
}

async fn list_entries(
    State(state): State<AppState>,
) -> Result<Json<Vec<Entry>>, (axum::http::StatusCode, String)> {
    let entries = get_all_entries(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(entries))
}

async fn get_all_users(pool: &sqlx::PgPool) -> Result<Vec<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        r#"
        SELECT id, nachname, email, status, token
        FROM schueler
        "#,
    )
    .fetch_all(pool)
    .await
}

async fn list_users(
    State(state): State<AppState>,
) -> Result<Json<Vec<User>>, (axum::http::StatusCode, String)> {
    let users = get_all_users(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(users))
}
