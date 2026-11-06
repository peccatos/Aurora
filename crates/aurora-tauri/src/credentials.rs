//! Native credential boundary for provider OAuth secrets.
//!
//! Secrets are kept exclusively in the operating-system credential store.
//! This module deliberately has no read-secret operation: Tauri callers can
//! store, remove, and inspect presence only.

use serde::Serialize;
use thiserror::Error;

const SERVICE: &str = "dev.aurora.player.oauth";

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("OS credential store unavailable: {0}")]
    Store(String),
    #[error("provider and account identifiers must not be empty")]
    InvalidIdentifier,
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

pub fn store(provider: &str, account_id: &str, secret: &str) -> Result<(), CredentialError> {
    if secret.is_empty() {
        return Err(CredentialError::Store("secret must not be empty".into()));
    }
    entry(provider, account_id)?
        .set_password(secret)
        .map_err(|e| CredentialError::Store(e.to_string()))
}

pub fn remove(provider: &str, account_id: &str) -> Result<(), CredentialError> {
    match entry(provider, account_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
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
        assert!(matches!(
            store("provider", "", "secret"),
            Err(CredentialError::InvalidIdentifier)
        ));
    }

    #[test]
    fn rejects_empty_secret() {
        assert!(matches!(
            store("provider", "account", ""),
            Err(CredentialError::Store(_))
        ));
    }
}
