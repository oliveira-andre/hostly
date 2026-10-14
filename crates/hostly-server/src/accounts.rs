//! Accounts and bearer-token sessions.

use std::sync::Arc;
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::Json;
use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hostly_protocol::{Account, Credentials, MIN_PASSWORD_LEN, Session, error_code};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{ApiFailure, ApiJson};
use crate::login_limits::{ClientIp, LoginGuard, LoginLimits, wait_text};

const MAX_EMAIL_LEN: usize = 254;

/// `POST /accounts`
pub async fn sign_up(
    State(pool): State<PgPool>,
    ApiJson(credentials): ApiJson<Credentials>,
) -> Result<(StatusCode, Json<Session>), ApiFailure> {
    let email = normalize_email(&credentials.email)?;
    if credentials.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiFailure::invalid(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let password_hash = hash_password(credentials.password).await?;
    let id = Uuid::new_v4();
    let inserted =
        sqlx::query("insert into accounts (id, email, password_hash) values ($1, $2, $3)")
            .bind(id)
            .bind(&email)
            .bind(&password_hash)
            .execute(&pool)
            .await;
    match inserted {
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(ApiFailure::new(
                StatusCode::CONFLICT,
                error_code::EMAIL_TAKEN,
                "an account with this email already exists",
            ));
        }
        result => result?,
    };
    let token = create_session(&pool, id).await?;
    Ok((
        StatusCode::CREATED,
        Json(Session {
            token,
            account: Account { id, email },
        }),
    ))
}

/// `POST /sessions`, within the [`LoginLimits`].
pub async fn log_in(
    State(pool): State<PgPool>,
    State(guard): State<Arc<LoginGuard>>,
    ClientIp(client): ClientIp,
    ApiJson(credentials): ApiJson<Credentials>,
) -> Result<Json<Session>, ApiFailure> {
    guard.begin(client)?;
    let email = credentials.email.trim().to_lowercase();
    // `locked_for` is the seconds left on the account's lock, if any.
    let account: Option<(Uuid, String, Option<f64>)> = sqlx::query_as(
        "select id, password_hash, extract(epoch from locked_until - now())::float8
         from accounts where email = $1",
    )
    .bind(&email)
    .fetch_optional(&pool)
    .await?;
    let invalid = || {
        ApiFailure::new(
            StatusCode::UNAUTHORIZED,
            error_code::INVALID_CREDENTIALS,
            "wrong email or password",
        )
    };
    let Some((id, password_hash, locked_for)) = account else {
        return Err(invalid());
    };
    if let Some(seconds) = locked_for.filter(|&seconds| seconds > 0.0) {
        let wait = Duration::from_secs_f64(seconds);
        return Err(ApiFailure::new(
            StatusCode::LOCKED,
            error_code::ACCOUNT_LOCKED,
            format!(
                "this account is locked after too many failed log-ins; try again in {}",
                wait_text(wait)
            ),
        )
        .retry_after(wait));
    }
    if !verify_password(credentials.password, password_hash).await? {
        record_failure(&pool, id, &guard.limits).await?;
        return Err(invalid());
    }
    guard.succeeded(client);
    sqlx::query(
        "update accounts set failed_logins = 0, locked_until = null
         where id = $1 and failed_logins > 0",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    let token = create_session(&pool, id).await?;
    Ok(Json(Session {
        token,
        account: Account { id, email },
    }))
}

/// `DELETE /sessions/current`: revokes the token used for this request.
pub async fn log_out(
    State(pool): State<PgPool>,
    auth: Authenticated,
) -> Result<StatusCode, ApiFailure> {
    sqlx::query("delete from sessions where token_hash = $1")
        .bind(&auth.token_hash)
        .execute(&pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /account`
pub async fn account(
    State(pool): State<PgPool>,
    auth: Authenticated,
) -> Result<Json<Account>, ApiFailure> {
    let email: String = sqlx::query_scalar("select email from accounts where id = $1")
        .bind(auth.account_id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(Account {
        id: auth.account_id,
        email,
    }))
}

/// The account behind the request's `Authorization: Bearer <token>` header.
pub struct Authenticated {
    pub account_id: Uuid,
    token_hash: Vec<u8>,
}

impl<S> FromRequestParts<S> for Authenticated
where
    PgPool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiFailure> {
        let pool = PgPool::from_ref(state);
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(ApiFailure::unauthorized)?;
        let token_hash = token_hash(token);
        let account_id: Option<Uuid> = sqlx::query_scalar(
            "update sessions set last_used_at = now() where token_hash = $1 returning account_id",
        )
        .bind(&token_hash)
        .fetch_optional(&pool)
        .await?;
        let account_id = account_id.ok_or_else(ApiFailure::unauthorized)?;
        Ok(Self {
            account_id,
            token_hash,
        })
    }
}

fn normalize_email(email: &str) -> Result<String, ApiFailure> {
    let email = email.trim().to_lowercase();
    let well_formed = email.len() <= MAX_EMAIL_LEN
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty());
    if well_formed {
        Ok(email)
    } else {
        Err(ApiFailure::invalid("email address is not valid"))
    }
}

/// Creates a session and returns its token; only the token's hash is stored.
async fn create_session(pool: &PgPool, account_id: Uuid) -> Result<String, ApiFailure> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(ApiFailure::internal)?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    sqlx::query("insert into sessions (token_hash, account_id) values ($1, $2)")
        .bind(token_hash(&token))
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(token)
}

/// Counts a wrong password for the account. Every `account_failures` of them in
/// a row lock it, each lock twice as long as the one before.
async fn record_failure(
    pool: &PgPool,
    account_id: Uuid,
    limits: &LoginLimits,
) -> Result<(), ApiFailure> {
    let every = i32::try_from(limits.account_failures.max(1)).unwrap_or(i32::MAX);
    let locked: bool = sqlx::query_scalar(
        "update accounts set
             failed_logins = failed_logins + 1,
             locked_until = case when (failed_logins + 1) % $2 = 0
                 then now() + make_interval(secs => least(
                     $3::float8 * power(2::float8, least((failed_logins + 1) / $2 - 1, 30)),
                     $4::float8))
                 else locked_until end
         where id = $1
         returning failed_logins % $2 = 0",
    )
    .bind(account_id)
    .bind(every)
    .bind(limits.account_lock.as_secs_f64())
    .bind(limits.max_account_lock.as_secs_f64())
    .fetch_one(pool)
    .await?;
    if locked {
        tracing::warn!(account = %account_id, "account locked after failed log-ins");
    }
    Ok(())
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

// Argon2 is deliberately slow, so it runs off the async workers.

async fn hash_password(password: String) -> Result<String, ApiFailure> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
    })
    .await
    .map_err(ApiFailure::internal)?
    .map_err(ApiFailure::internal)
}

async fn verify_password(password: String, stored: String) -> Result<bool, ApiFailure> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&stored).map_err(ApiFailure::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(ApiFailure::internal)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_trimmed_lowercased_and_checked() {
        assert_eq!(
            normalize_email("  Me@Example.COM ").unwrap(),
            "me@example.com"
        );
        for bad in [
            "",
            "no-at-sign",
            "@example.com",
            "me@",
            "two words@example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?} should be rejected");
        }
        let long = format!("{}@example.com", "a".repeat(MAX_EMAIL_LEN));
        assert!(normalize_email(&long).is_err());
    }

    #[tokio::test]
    async fn password_hashes_verify_only_the_original_password() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify_password("correct horse".into(), hash.clone())
                .await
                .unwrap()
        );
        assert!(!verify_password("wrong horse".into(), hash).await.unwrap());
    }
}
