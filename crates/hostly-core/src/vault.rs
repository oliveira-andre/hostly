//! End-to-end encryption of synced passwords and passphrases.
//!
//! - The account password never leaves the device. Argon2id turns it into a
//!   master key, and HKDF splits that into a login key (sent to the server in
//!   place of the password) and a wrapping key (never sent).
//! - A random vault key encrypts every synced secret. The server stores it
//!   only wrapped, so changing the password re-wraps it and nothing else.
//! - Each secret is sealed with XChaCha20-Poly1305, bound to its host's id so
//!   the server can't move it to another host.
//! - On the device, secrets are decrypted into the keychain as before, where
//!   the host editor can show and copy them.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hostly_protocol::MIN_PASSWORD_LEN;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Error, Result};

/// Keychain entry holding this device's copy of the vault key while logged in.
pub const VAULT_KEY_SECRET_KEY: &str = "sync/vault-key";

const SEALED_PREFIX: &str = "v1.";
const NONCE_LEN: usize = 24;
// Argon2id with 64 MiB and 3 passes: about a quarter second on a laptop.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;
const KDF_LANES: u32 = 1;

/// The keys derived from an account's email and password.
pub struct AccountKeys {
    /// Sent to the server as the password, with `KEY_VERSION_DERIVED`.
    pub login_key: String,
    wrap_key: [u8; 32],
}

impl AccountKeys {
    /// Slow on purpose; run it off the async workers.
    pub fn derive(email: &str, password: &str) -> Result<Self> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(Error::Vault(format!(
                "the password must be at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        // The normalized email is the salt, so the same password gives every
        // account different keys, and any device can derive them from the
        // email alone.
        let salt = Sha256::new()
            .chain_update(b"hostly/account-salt/v1\0")
            .chain_update(email.trim().to_lowercase().as_bytes())
            .finalize();
        let params = Params::new(KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES, Some(32))
            .map_err(|e| Error::Vault(e.to_string()))?;
        let mut master = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password.as_bytes(), &salt, &mut master)
            .map_err(|e| Error::Vault(e.to_string()))?;

        let hkdf = Hkdf::<Sha256>::new(None, &master);
        let mut login_key = [0u8; 32];
        let mut wrap_key = [0u8; 32];
        hkdf.expand(b"hostly/login-key/v1", &mut login_key)
            .and_then(|()| hkdf.expand(b"hostly/wrap-key/v1", &mut wrap_key))
            .map_err(|e| Error::Vault(e.to_string()))?;
        Ok(Self {
            login_key: URL_SAFE_NO_PAD.encode(login_key),
            wrap_key,
        })
    }
}

/// The account's key for its synced secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultKey([u8; 32]);

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKey(..)")
    }
}

impl VaultKey {
    pub fn generate() -> Result<Self> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| Error::Vault(e.to_string()))?;
        Ok(Self(key))
    }

    /// The vault key encrypted with the password's wrapping key, for the server.
    pub fn wrap(&self, keys: &AccountKeys) -> Result<String> {
        seal(&keys.wrap_key, b"hostly/vault-key/v1", &self.0)
    }

    /// Fails with [`Error::Vault`] when `keys` come from another password.
    pub fn unwrap(wrapped: &str, keys: &AccountKeys) -> Result<Self> {
        let bytes = open(&keys.wrap_key, b"hostly/vault-key/v1", wrapped)?;
        Ok(Self(bytes.try_into().map_err(|_| {
            Error::Vault("the vault key has the wrong length".into())
        })?))
    }

    /// For this device's keychain.
    pub fn to_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    pub fn from_base64(encoded: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded.trim())
            .map_err(|e| Error::Vault(e.to_string()))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            Error::Vault("the vault key has the wrong length".into())
        })?))
    }

    /// Encrypts a host's password or passphrase for syncing.
    pub fn seal_secret(&self, host_id: Uuid, secret: &str) -> Result<String> {
        seal(&self.0, &secret_context(host_id), secret.as_bytes())
    }

    /// Fails with [`Error::Vault`] if the secret was sealed with another key or for another host.
    pub fn open_secret(&self, host_id: Uuid, sealed: &str) -> Result<String> {
        let bytes = open(&self.0, &secret_context(host_id), sealed)?;
        String::from_utf8(bytes).map_err(|e| Error::Vault(e.to_string()))
    }
}

fn secret_context(host_id: Uuid) -> Vec<u8> {
    format!("hostly/secret/v1/{host_id}").into_bytes()
}

/// `v1.` + base64url(nonce ‖ ciphertext), authenticated together with `context`.
fn seal(key: &[u8; 32], context: &[u8], plaintext: &[u8]) -> Result<String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| Error::Vault(e.to_string()))?;
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: context,
            },
        )
        .map_err(|_| Error::Vault("couldn't encrypt".into()))?;
    let mut sealed = nonce.to_vec();
    sealed.extend(ciphertext);
    Ok(format!("{SEALED_PREFIX}{}", URL_SAFE_NO_PAD.encode(sealed)))
}

fn open(key: &[u8; 32], context: &[u8], sealed: &str) -> Result<Vec<u8>> {
    let undecryptable = || Error::Vault("couldn't decrypt; the key doesn't match".into());
    let bytes = sealed
        .strip_prefix(SEALED_PREFIX)
        .and_then(|body| URL_SAFE_NO_PAD.decode(body).ok())
        .filter(|bytes| bytes.len() > NONCE_LEN)
        .ok_or_else(|| Error::Vault("the encrypted data is malformed".into()))?;
    let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| undecryptable())?;
    XChaCha20Poly1305::new(&Key::from(*key))
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad: context,
            },
        )
        .map_err(|_| undecryptable())
}

#[cfg(test)]
mod tests {
    use hostly_protocol::validate_sealed;

    use super::*;

    const EMAIL: &str = "me@example.com";
    const PASSWORD: &str = "correct horse battery";

    #[test]
    fn keys_are_deterministic_per_email_and_password() {
        let a = AccountKeys::derive(EMAIL, PASSWORD).unwrap();
        let again = AccountKeys::derive(" Me@Example.COM ", PASSWORD).unwrap();
        assert_eq!(a.login_key, again.login_key);
        assert_eq!(a.wrap_key, again.wrap_key);
        // 32 bytes of base64url: long enough for the server's minimum.
        assert_eq!(a.login_key.len(), 43);
        assert_ne!(a.login_key.as_bytes(), &a.wrap_key[..]);

        let other_password = AccountKeys::derive(EMAIL, "another password").unwrap();
        let other_email = AccountKeys::derive("you@example.com", PASSWORD).unwrap();
        assert_ne!(a.login_key, other_password.login_key);
        assert_ne!(a.login_key, other_email.login_key);
    }

    #[test]
    fn short_passwords_are_rejected_before_deriving() {
        assert!(matches!(
            AccountKeys::derive(EMAIL, "short"),
            Err(Error::Vault(_))
        ));
    }

    #[test]
    fn the_vault_key_unwraps_only_with_the_same_password() {
        let keys = AccountKeys::derive(EMAIL, PASSWORD).unwrap();
        let vault = VaultKey::generate().unwrap();
        let wrapped = vault.wrap(&keys).unwrap();
        validate_sealed(&wrapped).unwrap();
        assert_eq!(VaultKey::unwrap(&wrapped, &keys).unwrap(), vault);

        let wrong = AccountKeys::derive(EMAIL, "wrong password!").unwrap();
        assert!(matches!(
            VaultKey::unwrap(&wrapped, &wrong),
            Err(Error::Vault(_))
        ));
        // Re-wrapping for a new password keeps the same vault key.
        let new_keys = AccountKeys::derive(EMAIL, "a new password").unwrap();
        let rewrapped = vault.wrap(&new_keys).unwrap();
        assert_eq!(VaultKey::unwrap(&rewrapped, &new_keys).unwrap(), vault);
    }

    #[test]
    fn secrets_open_only_for_their_host_and_key() {
        let vault = VaultKey::generate().unwrap();
        let host = Uuid::new_v4();
        let sealed = vault.seal_secret(host, "hunter2 ✓").unwrap();
        validate_sealed(&sealed).unwrap();
        assert!(!sealed.contains("hunter2"));
        // A fresh nonce every time.
        assert_ne!(sealed, vault.seal_secret(host, "hunter2 ✓").unwrap());
        assert_eq!(vault.open_secret(host, &sealed).unwrap(), "hunter2 ✓");

        assert!(vault.open_secret(Uuid::new_v4(), &sealed).is_err());
        let other = VaultKey::generate().unwrap();
        assert!(other.open_secret(host, &sealed).is_err());
        assert!(vault.open_secret(host, "v1.garbage").is_err());
    }

    #[test]
    fn the_vault_key_round_trips_through_the_keychain_format() {
        let vault = VaultKey::generate().unwrap();
        assert_eq!(VaultKey::from_base64(&vault.to_base64()).unwrap(), vault);
        assert!(VaultKey::from_base64("too-short").is_err());
        assert_eq!(format!("{vault:?}"), "VaultKey(..)");
    }
}
