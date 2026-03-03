pub mod health;
pub mod identity;
pub mod journal;
pub mod midi_state;
pub mod packets;
pub mod pipeline;
pub mod ringbuf;

/// Protocol version (v2 adds device_id for multi-device highways)
pub const PROTOCOL_VERSION: u8 = 2;

/// Maximum number of devices per host (limited by u16 device_mask bitmask)
pub const MAX_DEVICES_PER_HOST: u8 = 16;

/// Build info (set by build.rs from git)
pub const GIT_HASH: &str = env!("MIDINET_GIT_HASH");
pub const GIT_BRANCH: &str = env!("MIDINET_GIT_BRANCH");
pub const BUILD_TIME: &str = env!("MIDINET_BUILD_TIME");

/// Operational mode for the host daemon.
/// Configured in host.toml, requires restart to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationalMode {
    /// One device, no backup
    Single,
    /// Primary + backup of same device type with InputMux failover
    Redundant,
    /// N independent device highways
    #[serde(rename = "multi")]
    MultiDevice,
}

impl Default for OperationalMode {
    fn default() -> Self {
        Self::Single
    }
}

impl std::fmt::Display for OperationalMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single => write!(f, "single"),
            Self::Redundant => write!(f, "redundant"),
            Self::MultiDevice => write!(f, "multi"),
        }
    }
}

impl std::str::FromStr for OperationalMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "single" => Ok(Self::Single),
            "redundant" => Ok(Self::Redundant),
            "multi" | "multidevice" | "multi-device" | "multi_device" => Ok(Self::MultiDevice),
            _ => Err(format!("unknown operational mode: {s}")),
        }
    }
}

/// Human-readable version string: "v3.1 (abc1234)"
pub fn version_string() -> String {
    format!("{} ({})", GIT_BRANCH, GIT_HASH)
}

/// mDNS service type for MIDInet discovery
pub const MDNS_SERVICE_TYPE: &str = "_midinet._udp.local.";

/// Default multicast groups
pub const DEFAULT_PRIMARY_GROUP: &str = "239.69.83.1";
pub const DEFAULT_STANDBY_GROUP: &str = "239.69.83.2";
pub const DEFAULT_CONTROL_GROUP: &str = "239.69.83.100";

/// Default ports
pub const DEFAULT_DATA_PORT: u16 = 5004;
pub const DEFAULT_HEARTBEAT_PORT: u16 = 5005;
pub const DEFAULT_CONTROL_PORT: u16 = 5006;
pub const DEFAULT_FOCUS_PORT: u16 = 5007;
pub const DEFAULT_DISCOVERY_PORT: u16 = 5008;
pub const DEFAULT_ADMIN_PORT: u16 = 8080;
pub const DEFAULT_OSC_PORT: u16 = 5588;

/// Heartbeat defaults
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 3;
pub const DEFAULT_HEARTBEAT_MISS_THRESHOLD: u8 = 3;
