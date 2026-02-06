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
    // token -> last activity
    sessions: Arc<RwLock<HashMap<String, Instant>>>,
}

#[derive(Serialize, Deserialize, Debug, FromRow)]
struct Entry {
    id: i32,
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
        .connect("postgres://app:app@localhost:5432/app_db")
        .await?;

    let state = AppState {
        pool,
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/login/{token}", post(login_with_token))
        .route("/users", get(list_users))
        .route("/entries/{id}", get(get_entry_by_id))
        .route("/entries/{id}/contact", get(get_entry_contact))
        .route("/entries", get(list_entries).post(create_entry))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

// ---------- AUTH HELPERS ----------

fn read_bearer_token(headers: &HeaderMap) -> Result<&str, (StatusCode, String)> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Missing Authorization header".to_string(),
        ))?;

    let token = auth
        .strip_prefix("Bearer ")
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Expected: Bearer <token>".to_string(),
        ))?
        .trim();

    if token.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, "Empty token".to_string()));
    }

    Ok(token)
}

async fn token_exists(pool: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    let (exists,) =
        sqlx::query_as::<_, (bool,)>(r#"SELECT EXISTS(SELECT 1 FROM schueler WHERE token = $1)"#)
            .bind(token)
            .fetch_one(pool)
            .await?;

    Ok(exists)
}

/// Requires that:
/// 1) token is present in memory (user called /login/{token})
/// 2) last activity is not older than 10 minutes
/// Then it "touches" (refreshes) activity time.
async fn require_logged_in_and_touch(
    state: &AppState,
    token: &str,
) -> Result<(), (StatusCode, String)> {
    let idle_limit = Duration::from_secs(10 * 60);

    let mut sessions = state.sessions.write().await;

    let last = sessions.get(token).copied().ok_or((
        StatusCode::UNAUTHORIZED,
        "Not logged in. Call POST /login/{token}".to_string(),
    ))?;

    if last.elapsed() > idle_limit {
        sessions.remove(token); // expire session
        return Err((
            StatusCode::UNAUTHORIZED,
            "Session expired (idle > 10 min). Call POST /login/{token} again.".to_string(),
        ));
    }

    // touch
    sessions.insert(token.to_string(), Instant::now());
    Ok(())
}

// ---------- LOGIN ENDPOINT ----------

async fn login_with_token(
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // 1) token must exist in DB
    let exists = token_exists(&state.pool, &token)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !exists {
        return Err((StatusCode::UNAUTHORIZED, "Invalid token".to_string()));
    }

    // 2) mark as logged in (start session)
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(token, Instant::now());
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- DB QUERIES ----------

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
    let token = read_bearer_token(&headers)?;

    // must have called /login/{token} and be active within 10 min
    require_logged_in_and_touch(&state, token).await?;

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
    let token = read_bearer_token(&headers)?;
    require_logged_in_and_touch(&state, token).await?;

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
    let token = read_bearer_token(&headers)?;
    require_logged_in_and_touch(&state, token).await?;

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

    // Capacity rule
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
            // no special requirement
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "typ must be 'Angebot' or 'Anfrage'".to_string(),
            ));
        }
    }

    // optional: basic checks
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
    // Token-check: must be logged in + not idle
    let token = read_bearer_token(&headers)?;
    let user = get_user_by_token(token, &state.pool).await.unwrap();
    require_logged_in_and_touch(&state, token).await?;

    // Business validation
    validate_entry_payload(&payload)?;

    // Insert into DB
    // NOTE: adjust columns if your eintrag table differs (e.g. schueler_id)
    let inserted = sqlx::query_as::<_, Entry>(
        r#"
        INSERT INTO eintrag (titel, nachricht, typ, sitzplaetze, schueler_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, titel, nachricht, typ, sitzplaetze
        "#,
    )
    .bind(payload.titel)
    .bind(payload.nachricht)
    .bind(payload.typ)
    .bind(payload.sitzplaetze)
    .bind(user.id)
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
