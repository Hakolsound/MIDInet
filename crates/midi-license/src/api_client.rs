/// HTTP client for the licensing activation/validation API.
///
/// Calls Firebase Cloud Functions for:
/// - License activation (register machine)
/// - License deactivation (unregister machine)
/// - Periodic re-validation (refresh token)

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::storage::ActivationToken;

/// Base URL for the licensing API (Firebase Cloud Functions).
const DEFAULT_API_BASE: &str = "https://europe-west1-ledconfigtool.cloudfunctions.net";

/// Activate a license key on this machine.
pub async fn activate(
    api_base: Option<&str>,
    license_key: &str,
    machine_hash: &str,
    component: &str, // "host" or "client"
    hostname: &str,
    os: &str,
    version: &str,
) -> anyhow::Result<ActivationToken> {
    let base = api_base.unwrap_or(DEFAULT_API_BASE);
    let url = format!("{base}/activateLicense");

    let body = ActivateRequest {
        license_key: license_key.to_string(),
        machine_hash: machine_hash.to_string(),
        component: component.to_string(),
        hostname: hostname.to_string(),
        os: os.to_string(),
        version: version.to_string(),
    };

    debug!(url = %url, component, "Activating license");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to reach activation server")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Activation failed (HTTP {status}): {body}");
    }

    let token: ActivationToken = resp.json().await.context("Invalid activation response")?;
    debug!(license_id = %token.license_id, "Activation successful");
    Ok(token)
}

/// Deactivate this machine's license.
pub async fn deactivate(
    api_base: Option<&str>,
    license_key: &str,
    machine_hash: &str,
) -> anyhow::Result<()> {
    let base = api_base.unwrap_or(DEFAULT_API_BASE);
    let url = format!("{base}/deactivateLicense");

    let body = DeactivateRequest {
        license_key: license_key.to_string(),
        machine_hash: machine_hash.to_string(),
    };

    debug!(url = %url, "Deactivating license");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to reach activation server")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Deactivation failed (HTTP {status}): {body}");
    }

    debug!("Deactivation successful");
    Ok(())
}

/// Re-validate an existing activation (periodic check).
pub async fn validate(
    api_base: Option<&str>,
    license_key: &str,
    machine_hash: &str,
    current_token: &ActivationToken,
) -> anyhow::Result<ActivationToken> {
    let base = api_base.unwrap_or(DEFAULT_API_BASE);
    let url = format!("{base}/validateLicense");

    let body = ValidateRequest {
        license_key: license_key.to_string(),
        machine_hash: machine_hash.to_string(),
        server_signature: current_token.server_signature.clone(),
    };

    debug!(url = %url, "Re-validating license");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to reach validation server")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        warn!(status = %status, "Validation failed: {body_text}");
        anyhow::bail!("Validation failed (HTTP {status}): {body_text}");
    }

    let token: ActivationToken = resp.json().await.context("Invalid validation response")?;
    debug!("Re-validation successful");
    Ok(token)
}

#[derive(Serialize)]
struct ActivateRequest {
    license_key: String,
    machine_hash: String,
    component: String,
    hostname: String,
    os: String,
    version: String,
}

#[derive(Serialize)]
struct DeactivateRequest {
    license_key: String,
    machine_hash: String,
}

#[derive(Serialize)]
struct ValidateRequest {
    license_key: String,
    machine_hash: String,
    server_signature: String,
}

/// Response when the server returns an error with a message.
#[derive(Deserialize)]
pub struct ApiError {
    pub error: String,
    pub code: Option<String>,
}
