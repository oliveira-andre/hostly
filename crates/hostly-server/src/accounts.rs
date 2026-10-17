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
use hostly_protocol::{
    Account, Credentials, KEY_VERSION_DERIVED, KEY_VERSION_PLAIN, MIN_PASSWORD_LEN, NewAccount,
    PasswordChange, Session, error_code, validate_sealed,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{ApiFailure, ApiJson};
use crate::login_limits::{ClientIp, LoginGuard, LoginLimits, wait_text};

const MAX_EMAIL_LEN: usize = 254;

/// `POST /accounts`. Apps with end-to-end encryption send a derived login key
/// and the wrapped vault key; older apps send the password as typed.
pub async fn sign_up(
    State(pool): State<PgPool>,
    ApiJson(request): ApiJson<NewAccount>,
) -> Result<(StatusCode, Json<Session>), ApiFailure> {
    let NewAccount {
        credentials,
        vault_key,
    } = request;
    let email = normalize_email(&credentials.email)?;
    check_password(&credentials.password)?;
    let vault_key = match (credentials.key_version, vault_key) {
        (KEY_VERSION_PLAIN, None) => None,
        (KEY_VERSION_DERIVED, Some(vault_key)) => {
            validate_sealed(&vault_key)
                .map_err(|reason| ApiFailure::invalid(format!("vault key: {reason}")))?;
            Some(vault_key)
        }
        (KEY_VERSION_DERIVED, None) => {
            return Err(ApiFailure::invalid("a derived login key needs a vault key"));
        }
        (KEY_VERSION_PLAIN, Some(_)) => {
            return Err(ApiFailure::invalid("a vault key needs a derived login key"));
        }
        (version, _) => return Err(unknown_key_version(version)),
    };
    let password_hash = hash_password(credentials.password).await?;
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "insert into accounts (id, email, password_hash, key_version, vault_key)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&email)
    .bind(&password_hash)
    .bind(i16::from(credentials.key_version))
    .bind(&vault_key)
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
            vault_key,
        }),
    ))
}

/// What log-in reads about an account.
#[derive(sqlx::FromRow)]
struct StoredAccount {
    id: Uuid,
    password_hash: String,
    /// Seconds left on the account's lock, if any.
    locked_for: Option<f64>,
    key_version: i16,
    vault_key: Option<String>,
}

/// `POST /sessions`, within the [`LoginLimits`]. The password must come in the
/// form the account uses: an app with end-to-end encryption that sends a
/// derived key for an account still on its plain password is told so
/// ([`error_code::PLAIN_PASSWORD`]), without it counting as a failed log-in.
pub async fn log_in(
    State(pool): State<PgPool>,
    State(guard): State<Arc<LoginGuard>>,
    ClientIp(client): ClientIp,
    ApiJson(credentials): ApiJson<Credentials>,
) -> Result<Json<Session>, ApiFailure> {
    guard.begin(client)?;
    let email = credentials.email.trim().to_lowercase();
    let account: Option<StoredAccount> = sqlx::query_as(
        "select id, password_hash,
             extract(epoch from locked_until - now())::float8 as locked_for,
             key_version, vault_key
         from accounts where email = $1",
    )
    .bind(&email)
    .fetch_optional(&pool)
    .await?;
    let invalid = || invalid_credentials(credentials.key_version);
    let Some(StoredAccount {
        id,
        password_hash,
        locked_for,
        key_version,
        vault_key,
    }) = account
    else {
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
    if key_version == i16::from(KEY_VERSION_PLAIN) && credentials.key_version == KEY_VERSION_DERIVED
    {
        guard.succeeded(client);
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            error_code::PLAIN_PASSWORD,
            "this account still uses its plain password; log in with it once to upgrade",
        ));
    }
    let matches = key_version == i16::from(credentials.key_version)
        && verify_password(credentials.password, password_hash).await?;
    if !matches {
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
        vault_key,
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

/// `PUT /account/password`: needs the current password, in the form the
/// account uses now. Sets a derived login key and the re-wrapped vault key,
/// which also upgrades an account from its plain password. Sessions stay
/// valid. A wrong current password counts towards the account's lock.
pub async fn change_password(
    State(pool): State<PgPool>,
    State(guard): State<Arc<LoginGuard>>,
    auth: Authenticated,
    ApiJson(change): ApiJson<PasswordChange>,
) -> Result<StatusCode, ApiFailure> {
    check_password(&change.new_password)?;
    validate_sealed(&change.vault_key)
        .map_err(|reason| ApiFailure::invalid(format!("vault key: {reason}")))?;
    let (password_hash, key_version): (String, i16) =
        sqlx::query_as("select password_hash, key_version from accounts where id = $1")
            .bind(auth.account_id)
            .fetch_one(&pool)
            .await?;
    let matches = key_version == i16::from(change.current_key_version)
        && verify_password(change.current_password, password_hash).await?;
    if !matches {
        record_failure(&pool, auth.account_id, &guard.limits).await?;
        return Err(ApiFailure::new(
            StatusCode::UNAUTHORIZED,
            error_code::INVALID_CREDENTIALS,
            "the current password is wrong",
        ));
    }
    let password_hash = hash_password(change.new_password).await?;
    sqlx::query(
        "update accounts set password_hash = $2, key_version = $3, vault_key = $4,
             failed_logins = 0, locked_until = null
         where id = $1",
    )
    .bind(auth.account_id)
    .bind(&password_hash)
    .bind(i16::from(KEY_VERSION_DERIVED))
    .bind(&change.vault_key)
    .execute(&pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
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

fn check_password(password: &str) -> Result<(), ApiFailure> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiFailure::invalid(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

fn unknown_key_version(version: u8) -> ApiFailure {
    ApiFailure::invalid(format!("unknown key version {version}"))
}

/// The same answer whether the email is unknown or the password wrong. An app
/// from before end-to-end encryption also gets it for an upgraded account,
/// with a hint to update.
fn invalid_credentials(key_version: u8) -> ApiFailure {
    let message = if key_version == KEY_VERSION_PLAIN {
        "wrong email or password. If this account was upgraded to end-to-end encryption, update Hostly on this device"
    } else {
        "wrong email or password"
    };
    ApiFailure::new(
        StatusCode::UNAUTHORIZED,
        error_code::INVALID_CREDENTIALS,
        message,
    )
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
