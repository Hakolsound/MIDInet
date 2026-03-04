/// License key parsing and Ed25519 signature verification.
///
/// Key format: `MIDINET-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX`
/// The payload is base32-encoded and contains a signed license blob.

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, VerifyingKey, Verifier};
use tracing::debug;

use crate::state::{FeatureFlags, LicenseTier};

/// Compiled-in Ed25519 public key (32 bytes).
const PUBLIC_KEY_BYTES: &[u8; 32] = include_bytes!("../../midi-protocol/license_public_key.bin");

/// Total trial budget: 120 minutes = 7200 seconds.
pub const TRIAL_BUDGET_SECS: u64 = 7200;

/// Parsed and verified license payload.
#[derive(Debug, Clone)]
pub struct LicensePayload {
    pub version: u8,
    pub tier: LicenseTier,
    pub max_hosts: u8,
    pub max_clients: u8,
    pub features: FeatureFlags,
    pub issued_at: u32,
    pub expires_at: u32, // 0 = never expires (pioneer)
    pub license_id: [u8; 16],
}

/// Size of the signed data (everything before the 64-byte signature).
const PAYLOAD_SIZE: usize = 1 + 1 + 1 + 1 + 2 + 4 + 4 + 16; // = 30 bytes
const SIGNATURE_SIZE: usize = 64;
const TOTAL_SIZE: usize = PAYLOAD_SIZE + SIGNATURE_SIZE; // = 94 bytes

impl LicensePayload {
    /// Parse and verify a license key string.
    ///
    /// Returns `None` if the key is malformed, the signature is invalid,
    /// or the tier byte is unknown.
    pub fn from_key(key: &str) -> Option<Self> {
        // Strip prefix and dashes
        let stripped: String = key
            .trim()
            .strip_prefix("MIDINET-")
            .unwrap_or(key.trim())
            .chars()
            .filter(|c| *c != '-')
            .collect();

        // Decode base32
        let raw = BASE32_NOPAD.decode(stripped.as_bytes()).ok()?;
        if raw.len() != TOTAL_SIZE {
            debug!(
                len = raw.len(),
                expected = TOTAL_SIZE,
                "License key: unexpected payload size"
            );
            return None;
        }

        // Split payload and signature
        let (payload_bytes, sig_bytes) = raw.split_at(PAYLOAD_SIZE);
        let sig = Signature::from_slice(sig_bytes).ok()?;

        // Verify signature
        let verifying_key = VerifyingKey::from_bytes(PUBLIC_KEY_BYTES).ok()?;
        verifying_key.verify(payload_bytes, &sig).ok()?;

        // Parse payload fields
        let version = payload_bytes[0];
        if version != 1 {
            debug!(version, "License key: unsupported version");
            return None;
        }

        let tier = LicenseTier::from_u8(payload_bytes[1])?;
        let max_hosts = payload_bytes[2];
        let max_clients = payload_bytes[3];
        let features = FeatureFlags(u16::from_le_bytes([payload_bytes[4], payload_bytes[5]]));
        let issued_at = u32::from_le_bytes([
            payload_bytes[6],
            payload_bytes[7],
            payload_bytes[8],
            payload_bytes[9],
        ]);
        let expires_at = u32::from_le_bytes([
            payload_bytes[10],
            payload_bytes[11],
            payload_bytes[12],
            payload_bytes[13],
        ]);
        let mut license_id = [0u8; 16];
        license_id.copy_from_slice(&payload_bytes[14..30]);

        Some(Self {
            version,
            tier,
            max_hosts,
            max_clients,
            features,
            issued_at,
            expires_at,
            license_id,
        })
    }

    /// Check if the update entitlement has expired.
    /// Returns `None` if the license never expires (pioneer).
    pub fn update_expires_in_secs(&self) -> Option<i64> {
        if self.expires_at == 0 {
            return None; // never expires
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Some(self.expires_at as i64 - now)
    }

    /// Hex-encoded license_id for API calls and display.
    pub fn license_id_hex(&self) -> String {
        self.license_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Encode a license payload and sign it with the given private key bytes.
/// Used by the Cloud Functions (via WASM or as a reference implementation).
/// In production, signing is done server-side; this is for tests.
#[cfg(test)]
pub fn sign_license_payload(
    tier: LicenseTier,
    max_hosts: u8,
    max_clients: u8,
    features: FeatureFlags,
    issued_at: u32,
    expires_at: u32,
    license_id: [u8; 16],
    signing_key: &ed25519_dalek::SigningKey,
) -> String {
    use ed25519_dalek::Signer;

    let mut payload = Vec::with_capacity(PAYLOAD_SIZE);
    payload.push(1u8); // version
    payload.push(tier.to_u8());
    payload.push(max_hosts);
    payload.push(max_clients);
    payload.extend_from_slice(&features.0.to_le_bytes());
    payload.extend_from_slice(&issued_at.to_le_bytes());
    payload.extend_from_slice(&expires_at.to_le_bytes());
    payload.extend_from_slice(&license_id);

    let signature = signing_key.sign(&payload);
    payload.extend_from_slice(&signature.to_bytes());

    let encoded = BASE32_NOPAD.encode(&payload);

    // Format as MIDINET-XXXX-XXXX-...
    let chunks: Vec<&str> = encoded.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("MIDINET-{}", chunks.join("-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn test_keypair() -> SigningKey {
        // Load the actual private key generated for this project
        let raw = include_bytes!("../../../keys/license_private_key_raw.bin");
        SigningKey::from_bytes(raw)
    }

    #[test]
    fn roundtrip_sign_verify() {
        let sk = test_keypair();
        let license_id = [1u8; 16];

        let key_str = sign_license_payload(
            LicenseTier::Pro,
            2,
            255,
            FeatureFlags(FeatureFlags::ALL),
            1700000000, // issued_at
            1731536000, // expires_at (12 months later)
            license_id,
            &sk,
        );

        assert!(key_str.starts_with("MIDINET-"));

        let payload = LicensePayload::from_key(&key_str).expect("should parse and verify");
        assert_eq!(payload.version, 1);
        assert_eq!(payload.tier, LicenseTier::Pro);
        assert_eq!(payload.max_hosts, 2);
        assert_eq!(payload.max_clients, 255);
        assert!(payload.features.has(FeatureFlags::REDUNDANT_MODE));
        assert!(payload.features.has(FeatureFlags::MULTI_DEVICE));
        assert_eq!(payload.license_id, license_id);
    }

    #[test]
    fn invalid_key_rejected() {
        assert!(LicensePayload::from_key("MIDINET-AAAA-BBBB-CCCC").is_none());
        assert!(LicensePayload::from_key("not-a-key").is_none());
        assert!(LicensePayload::from_key("").is_none());
    }

    #[test]
    fn tampered_key_rejected() {
        let sk = test_keypair();
        let key_str = sign_license_payload(
            LicenseTier::Solo,
            1,
            3,
            FeatureFlags(0),
            1700000000,
            1731536000,
            [2u8; 16],
            &sk,
        );

        // Tamper with one character
        let mut chars: Vec<char> = key_str.chars().collect();
        // Change a character in the payload area (after "MIDINET-")
        let idx = 8; // first char of payload
        chars[idx] = if chars[idx] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();

        assert!(LicensePayload::from_key(&tampered).is_none());
    }
}
