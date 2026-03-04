/// Activation token handling.
///
/// The activation token is a JSON blob returned by the licensing server
/// after successful activation. It's cached locally and checked on startup
/// to allow offline operation with a 30-day grace period.

use crate::storage::ActivationToken;

/// Validate the cached activation token.
///
/// Returns `Ok(())` if the token is valid and within the offline grace period.
/// Returns `Err` with the reason if the token is invalid or expired.
pub fn validate_cached_token(
    token: &ActivationToken,
    expected_machine_hash: &str,
) -> Result<(), TokenError> {
    // Check machine binding
    if token.machine_hash != expected_machine_hash {
        return Err(TokenError::MachineMismatch);
    }

    // Check offline grace period
    let since_validation = token.seconds_since_validation();
    if since_validation > ActivationToken::OFFLINE_GRACE_SECS {
        return Err(TokenError::OfflineGraceExceeded {
            days_offline: since_validation / 86400,
        });
    }

    Ok(())
}

/// Errors from activation token validation.
#[derive(Debug)]
pub enum TokenError {
    /// Token was generated for a different machine.
    MachineMismatch,
    /// Too long since last server validation.
    OfflineGraceExceeded { days_offline: u64 },
}
