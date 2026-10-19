//! Versioned Linux Bluetooth helper contract shared by preparation and execution.
//!
//! The contract covers only the privileged helper's interface. The helper
//! reports what the target answered; the unprivileged runner judges it against
//! the target profile, so a target change never requires a reinstallation.

pub const CONNECTION_RESET_SCHEMA: u32 = 14;
pub const HELPER_CAPABILITIES: &str = "schema=21 dtm-check=v1,v2 dtm-profile=receive-transmit,transmit,receive-silence encrypted-acl=true key-refresh=true security-failure=missing-key,wrong-key,missing-refresh-key,active-data-mic post-rejection-version=true termination=peer-reset,peer-rfkill,target-disconnect,target-reset att-parameters=7.5ms,restore";
