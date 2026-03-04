/// License state types.

use serde::{Deserialize, Serialize};

/// Current license state of the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LicenseState {
    /// Fully licensed, all features available
    Licensed {
        tier: LicenseTier,
        /// Seconds until update entitlement expires (None = never/pioneer)
        update_expires_in_secs: Option<i64>,
    },
    /// Trial mode, countdown active
    Trial {
        /// Remaining seconds of trial
        remaining_secs: u64,
        /// Total trial budget in seconds (7200 = 120 min)
        total_secs: u64,
    },
    /// Trial expired or license invalid — degraded mode
    Degraded { reason: DegradedReason },
    /// First run, no license or trial yet
    Unlicensed,
}

impl LicenseState {
    pub fn is_licensed(&self) -> bool {
        matches!(self, Self::Licensed { .. })
    }

    pub fn is_trial(&self) -> bool {
        matches!(self, Self::Trial { .. })
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self, Self::Degraded { .. })
    }

    /// Short label for logs and UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Licensed { .. } => "licensed",
            Self::Trial { .. } => "trial",
            Self::Degraded { .. } => "degraded",
            Self::Unlicensed => "unlicensed",
        }
    }
}

/// License tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LicenseTier {
    Solo,
    Pro,
    Fleet,
    /// Free license for early adopters
    Pioneer,
}

impl LicenseTier {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Solo),
            2 => Some(Self::Pro),
            3 => Some(Self::Fleet),
            255 => Some(Self::Pioneer),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Solo => 1,
            Self::Pro => 2,
            Self::Fleet => 3,
            Self::Pioneer => 255,
        }
    }

    pub fn max_hosts(self) -> u8 {
        match self {
            Self::Solo => 1,
            Self::Pro => 2,
            Self::Fleet => 3,
            Self::Pioneer => 2, // same as Pro
        }
    }

    pub fn max_clients(self) -> u8 {
        match self {
            Self::Solo => 3,
            Self::Pro | Self::Fleet | Self::Pioneer => 255,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Solo => "Solo",
            Self::Pro => "Pro",
            Self::Fleet => "Fleet",
            Self::Pioneer => "Pioneer",
        }
    }
}

/// Feature flags encoded as a bitmask in the license key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureFlags(pub u16);

impl FeatureFlags {
    pub const REDUNDANT_MODE: u16 = 1 << 0;
    pub const MULTI_DEVICE: u16 = 1 << 1;
    pub const ALL: u16 = Self::REDUNDANT_MODE | Self::MULTI_DEVICE;

    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    /// Solo gets no extra features; Pro/Fleet/Pioneer get all.
    pub fn for_tier(tier: LicenseTier) -> Self {
        match tier {
            LicenseTier::Solo => Self(0),
            _ => Self(Self::ALL),
        }
    }
}

/// Why the license is in degraded mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradedReason {
    TrialExpired,
    LicenseExpired,
    ActivationRevoked,
    TamperDetected,
    OfflineGraceExceeded,
}
