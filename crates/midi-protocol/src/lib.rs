pub mod health;
pub mod identity;
pub mod journal;
pub mod midi_state;
pub mod packets;
pub mod pipeline;
pub mod ringbuf;

/// Protocol version
pub const PROTOCOL_VERSION: u8 = 1;

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
pub const DEFAULT_OSC_PORT: u16 = 8000;

/// Heartbeat defaults
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 3;
pub const DEFAULT_HEARTBEAT_MISS_THRESHOLD: u8 = 3;
