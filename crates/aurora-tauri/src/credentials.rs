//! Native credential boundary for provider OAuth secrets.
//!
//! Token sets are serialized only inside this module and stored in the OS
//! keyring. No command accepts or returns credential material.

use aurora_providers::token::TokenSet;
use serde::Serialize;
use thiserror::Error;

const SERVICE: &str = "dev.aurora.player.oauth";

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("OS credential store unavailable: {0}")]
    Store(String),
    #[error("provider and account identifiers must not be empty")]
    InvalidIdentifier,
    #[error("stored credential is invalid")]
    InvalidCredential,
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialStatus {
    pub provider: String,
    pub account_id: String,
    pub present: bool,
}

fn entry(provider: &str, account_id: &str) -> Result<keyring::Entry, CredentialError> {
    if provider.trim().is_empty() || account_id.trim().is_empty() {
        return Err(CredentialError::InvalidIdentifier);
    }
    let username = format!("{provider}:{account_id}");
    keyring::Entry::new(SERVICE, &username).map_err(|e| CredentialError::Store(e.to_string()))
}

/// Store the complete token set. This is deliberately not exposed as a Tauri
/// command; only native OAuth/API code may call it.
pub(crate) fn store_token_set(
    provider: &str,
    account_id: &str,
    tokens: &TokenSet,
) -> Result<(), CredentialError> {
    let encoded = tokens
        .encode_native()
        .map_err(|_| CredentialError::InvalidCredential)?;
    let value = String::from_utf8(encoded).map_err(|_| CredentialError::InvalidCredential)?;
    entry(provider, account_id)?
        .set_password(&value)
        .map_err(|e| CredentialError::Store(e.to_string()))
}

pub fn remove(provider: &str, account_id: &str) -> Result<(), CredentialError> {
    match entry(provider, account_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CredentialError::Store(e.to_string())),
    }
}

pub fn status(provider: &str, account_id: &str) -> Result<CredentialStatus, CredentialError> {
    let present = match entry(provider, account_id)?.get_password() {
        Ok(_) => true,
        Err(keyring::Error::NoEntry) => false,
        Err(e) => return Err(CredentialError::Store(e.to_string())),
    };
    Ok(CredentialStatus {
        provider: provider.to_owned(),
        account_id: account_id.to_owned(),
        present,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_identifiers_without_touching_store() {
        assert!(matches!(
            status("", "account"),
            Err(CredentialError::InvalidIdentifier)
        ));
    }
    #[test]
    fn token_set_round_trips_without_frontend_dto() {
        let tokens = TokenSet::decode_native(br#"{"access_token":"a","token_type":"Bearer","refresh_token":"r","scope":null,"expires_at_unix":4102444800}"#).unwrap();
        let encoded = tokens.encode_native().unwrap();
        let restored = TokenSet::decode_native(&encoded).unwrap();
        assert_eq!(restored.access_token, "a");
        assert_eq!(restored.refresh_token.as_deref(), Some("r"));
    }
}
