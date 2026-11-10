//! Email/password accounts: register, login, logout, me.
//!
//! Passwords are stored as Argon2id PHC strings (default params). Login
//! performs the same Argon2 work whether or not the user exists, so response
//! timing does not leak account enumeration. A login session is a random
//! 128-bit id in the `__Host-aurora_auth` cookie (30 days); its hash is the
//! `login_sessions` primary key, exactly the at-rest-hashing pattern the
//! OAuth tables use. The CSRF cookie is shared with the OAuth surface so the
//! frontend has exactly one token to echo.
//!
//! Grant claiming: registering (or logging in) from a browser that carries
//! the anonymous `__Host-aurora_user` cookie moves that cookie's OAuth
//! grants to the account's stable hash (`hash("user:{id}")`). This is gated
//! on `origin_ok` -- a cross-site attacker cannot drive the claim from their
//! own page, and SameSite=Lax keeps the cookies from being sent cross-site
//! anyway.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CSRF_HEADER, PeerAddr, cookie_header, cookie_value, csrf_cookie, fail, guard, hash,
    limits, origin_ok, random, user_token,
};

/// The login-session cookie. Scoped exactly like the OAuth session cookie,
/// but 30 days instead of 8 hours.
const AUTH_COOKIE: &str = "__Host-aurora_auth";
/// Login session lifetime: 30 days.
const LOGIN_SESSION_DAYS: i64 = 30;
/// Rate-limit bucket shared by every auth endpoint.
const PROVIDER: &str = "auth";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credentials {
    email: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthBody {
    email: String,
    csrf_token: String,
}

/// An email address shape we are willing to store: bounded, ASCII-graphic,
/// and containing an `@`. This is a mailbox-shape sanity check, not RFC 5322
/// validation -- the address is never used to send mail, only to log in.
fn is_email(raw: &str) -> bool {
    (3..=254).contains(&raw.len())
        && raw.contains('@')
        && raw.bytes().all(|b| b.is_ascii_graphic() && b != b' ')
}

/// A password we are willing to hash: 10..=128 bytes. Byte length (not char
/// count) bounds what Argon2 processes; multi-byte characters simply cost
/// more of the budget.
fn is_password(raw: &str) -> bool {
    (10..=128).contains(&raw.len())
}

/// Argon2id hash of a password, as a PHC string.
fn hash_password(password: &str) -> Result<String, ()> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| ())
}

/// Constant-shape credential check: `Ok(true)`/`Ok(false)` on match, and the
/// *same* Argon2 work when the stored hash is absent (unknown user) as when
/// it is present, so timing does not reveal which emails are registered.
fn verify_password(stored: Option<&str>, password: &str) -> Result<bool, ()> {
    match stored {
        Some(stored) => {
            let parsed = PasswordHash::new(stored).map_err(|_| ())?;
            // A malformed stored hash is answered by Argon2's error, not
            // short-circuited: the outcome is still just "wrong password".
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        }
        None => {
            // Unknown user: hash the submitted password anyway (same cost),
            // then report failure.
            hash_password(password)?;
            Ok(false)
        }
    }
}

/// Mints a login session for `user_id`, returning the response to send
/// (status + cookies + body). On storage failure returns the 500 to send.
#[allow(clippy::result_large_err)]
async fn mint_session(
    state: &AppState,
    headers: &HeaderMap,
    status: StatusCode,
    user_id: i64,
    email: &str,
) -> Result<Response, Response> {
    let session_id = random();
    let csrf_token = random();
    let expires = chrono::Utc::now() + chrono::Duration::days(LOGIN_SESSION_DAYS);
    if aurora_storage::insert_login_session(
        &state.pool,
        &hash(&session_id),
        user_id,
        &hash(&csrf_token),
        &expires.to_rfc3339(),
    )
    .await
    .is_err()
    {
        return Err(fail(StatusCode::INTERNAL_SERVER_ERROR));
    }
    // Claim the anonymous user cookie's grants into the account's hash, so
    // OAuth accounts connected before registering stay reachable. The caller
    // has already verified the origin: the browser that owns these cookies
    // is the one asking.
    if let Some(anonymous) = user_token(headers)
        && aurora_storage::claim_grants(
            &state.pool,
            &hash(&anonymous),
            &hash(&format!("user:{user_id}")),
        )
        .await
        .is_err()
    {
        return Err(fail(StatusCode::INTERNAL_SERVER_ERROR));
    }
    let mut response = (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(AuthBody {
            email: email.to_owned(),
            csrf_token: csrf_token.clone(),
        }),
    )
        .into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(&format!(
            "{AUTH_COOKIE}={session_id}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={}",
            LOGIN_SESSION_DAYS * 24 * 3600
        )),
    );
    response
        .headers_mut()
        .append(header::SET_COOKIE, cookie_header(&csrf_cookie(&csrf_token)));
    Ok(response)
}

/// `POST /api/auth/register`.
pub(crate) async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
    body: Json<Credentials>,
) -> Response {
    let Json(credentials) = body;
    if !state.config.registration_open {
        return fail(StatusCode::FORBIDDEN);
    }
    if !origin_ok(&headers, &state.config) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = crate::rate_limit(&state, guard::client_ip(peer.0), PROVIDER) {
        return rejection;
    }
    let email = credentials.email.trim().to_ascii_lowercase();
    if !is_email(&email) || !is_password(&credentials.password) {
        return fail(StatusCode::BAD_REQUEST);
    }
    let phc = match hash_password(&credentials.password) {
        Ok(x) => x,
        Err(()) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    match aurora_storage::insert_user(&state.pool, &email, &phc).await {
        Ok(Some(user_id)) => {
            match mint_session(&state, &headers, StatusCode::CREATED, user_id, &email).await {
                Ok(response) => response,
                Err(rejection) => rejection,
            }
        }
        // A UNIQUE constraint hit surfaces as `Ok(None)`: the email is taken.
        Ok(None) => fail(StatusCode::CONFLICT),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `POST /api/auth/login`.
pub(crate) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
    body: Json<Credentials>,
) -> Response {
    let Json(credentials) = body;
    if !origin_ok(&headers, &state.config) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = crate::rate_limit(&state, guard::client_ip(peer.0), PROVIDER) {
        return rejection;
    }
    let email = credentials.email.trim().to_ascii_lowercase();
    // The lookup runs regardless of input shape so the verify path is
    // reached (and its cost paid) even for a malformed email.
    let user = match aurora_storage::get_user_by_email(&state.pool, &email).await {
        Ok(x) => x,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let stored_hash = match &user {
        Some(u) => stored_password_hash(&state, u.id).await,
        None => None,
    };
    let matched = match verify_password(stored_hash.as_deref(), &credentials.password) {
        Ok(x) => x,
        Err(()) => {
            // Malformed stored hash or Argon2 failure: same answer as wrong
            // credentials, never a 500 that distinguishes the account.
            return fail(StatusCode::UNAUTHORIZED);
        }
    };
    let Some(user) = user else {
        // Unknown email: the dummy Argon2 work already ran inside
        // verify_password; the 401 is identical to a password mismatch.
        return fail(StatusCode::UNAUTHORIZED);
    };
    if !matched {
        return fail(StatusCode::UNAUTHORIZED);
    }
    match mint_session(&state, &headers, StatusCode::OK, user.id, &user.email).await {
        Ok(response) => response,
        Err(rejection) => rejection,
    }
}

/// Reads the stored password hash for a user id. Fetched only inside the
/// login handler so the hash never travels further than the one verify call.
async fn stored_password_hash(state: &AppState, user_id: i64) -> Option<String> {
    aurora_storage::get_user_password_hash(&state.pool, user_id)
        .await
        .ok()
        .flatten()
}

/// The login-session cookie value, when present and well-formed.
pub(crate) fn auth_token(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, AUTH_COOKIE).filter(|id| limits::is_user_token(id))
}

/// The user a live login session belongs to (session row checked against
/// `now`), as `(user_id, email)`.
pub(crate) async fn login_user(state: &AppState, headers: &HeaderMap) -> Option<(i64, String)> {
    let session_id = auth_token(headers)?;
    let user_id = aurora_storage::get_login_session(
        &state.pool,
        &hash(&session_id),
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
    .ok()
    .flatten()?;
    aurora_storage::get_user_by_id(&state.pool, user_id)
        .await
        .ok()
        .flatten()
        .map(|u| (u.id, u.email))
}

/// Validates the login-CSRF pair: the header token must hash-match the
/// `csrf_hash` on the login-session row for the presented auth cookie.
#[allow(clippy::result_large_err)]
async fn validate_login_csrf(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    if !origin_ok(headers, &state.config) {
        return Err(fail(StatusCode::FORBIDDEN));
    }
    let session_id = match auth_token(headers) {
        Some(x) => x,
        None => return Err(fail(StatusCode::UNAUTHORIZED)),
    };
    let token = match headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|x| limits::is_csrf_token(x))
    {
        Some(x) => x.to_owned(),
        None => return Err(fail(StatusCode::FORBIDDEN)),
    };
    match aurora_storage::validate_login_csrf(
        &state.pool,
        &hash(&session_id),
        &hash(&token),
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
    {
        Ok(true) => Ok(session_id),
        Ok(false) => Err(fail(StatusCode::FORBIDDEN)),
        Err(_) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

/// `POST /api/auth/logout`.
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if let Some(rejection) = crate::rate_limit(&state, guard::client_ip(peer.0), PROVIDER) {
        return rejection;
    }
    let session_id = match validate_login_csrf(&state, &headers).await {
        Ok(x) => x,
        Err(rejection) => return rejection,
    };
    if aurora_storage::delete_login_session(&state.pool, &hash(&session_id))
        .await
        .is_err()
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(&format!(
            "{AUTH_COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
        )),
    );
    response
}

/// `GET /api/auth/me`.
pub(crate) async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if !origin_ok(&headers, &state.config) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = crate::rate_limit(&state, guard::client_ip(peer.0), PROVIDER) {
        return rejection;
    }
    let body = match login_user(&state, &headers).await {
        Some((_, email)) => serde_json::json!({"authenticated": true, "email": email}),
        None => serde_json::json!({"authenticated": false}),
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}
