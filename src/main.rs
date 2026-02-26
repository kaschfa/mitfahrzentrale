use axum::extract::Path;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct Entry {
    id: i64,
    titel: String,
    nachricht: String,
    typ: String,
    sitzplaetze: i32,
}

#[derive(Deserialize, Debug)]
struct NewEntry {
    titel: String,
    nachricht: String,
    typ: String,
    sitzplaetze: i32,
    // if you have it in your DB:
    // schueler_id: i32,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct User {
    id: i32,
    nachname: String,
    email: String,
    status: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct EntryContact {
    email: String,
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect("postgres://student_user:strong_password@static.170.127.180.157.clients.your-server.de:80/mitfahrzentrale_db")
        .await?;

    let state = AppState {
        pool,
    };

    let app = Router::new()
        .route("/users", get(list_users))
        .route("/entries/{id}", get(get_entry_by_id))
        .route("/entries/{id}/contact", get(get_entry_contact))
        .route("/entries", get(list_entries).post(create_entry))
        .with_state(state);

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

// ---------- HANDLERS ----------

async fn list_entries(
    State(state): State<AppState>,
) -> Result<Json<Vec<Entry>>, (StatusCode, String)> {
    let entries = get_all_entries(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(entries))
}

async fn list_users(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<User>>, (StatusCode, String)> {

    let users = get_all_users(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(users))
}

async fn get_entry_by_id(
    Path(id): Path<i32>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Entry>, (StatusCode, String)> {

    let entry = sqlx::query_as::<_, Entry>(
        r#"
            SELECT id, titel, nachricht, typ, sitzplaetze
            FROM eintrag
            WHERE id=$1
        "#,
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(entry))
}

async fn get_entry_contact(
    Path(id): Path<i32>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<EntryContact>, (StatusCode, String)> {

    let contact = sqlx::query_as::<_, EntryContact>(
        r#"
            SELECT schueler.email
            FROM eintrag
            inner join schueler on eintrag.schueler_id=schueler.id where eintrag.id=$1;
        "#,
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(contact))
}

fn validate_entry_payload(p: &NewEntry) -> Result<(), (StatusCode, String)> {
    // Content filter (case-insensitive)
    let msg = p.nachricht.to_lowercase();
    if msg.contains("werbung") || msg.contains("verkauf") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Entry rejected: advertising/selling content".to_string(),
        ));
    }

    match p.typ.as_str() {
        "Angebot" => {
            if p.sitzplaetze <= 0 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Typ 'Angebot' requires sitzplaetze > 0".to_string(),
                ));
            }
        }
        "Anfrage" => {
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "typ must be 'Angebot' or 'Anfrage'".to_string(),
            ));
        }
    }

    if p.titel.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "titel must not be empty".to_string(),
        ));
    }
    if p.nachricht.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "nachricht must not be empty".to_string(),
        ));
    }

    Ok(())
}

async fn create_entry(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(payload): Json<NewEntry>,
) -> Result<(StatusCode, Json<Entry>), (StatusCode, String)> {

    validate_entry_payload(&payload)?;

    let inserted = sqlx::query_as::<_, Entry>(
        r#"
        INSERT INTO eintrag (titel, nachricht, typ, sitzplaetze, schueler_id)
        VALUES ($1, $2, $3, $4)
        RETURNING id, titel, nachricht, typ, sitzplaetze
        "#,
    )
    .bind(payload.titel)
    .bind(payload.nachricht)
    .bind(payload.typ)
    .bind(payload.sitzplaetze)
    //.bind(user.id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::CREATED, Json(inserted)))
}

async fn get_user_by_token(token: &str, pool: &PgPool) -> Result<User, (StatusCode, String)> {
    let user = sqlx::query_as::<_, User>(
        r#"
            SELECT * from schueler
            WHERE token=$1
        "#,
    )
    .bind(token)
    .fetch_one(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(user)
}
