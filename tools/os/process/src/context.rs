//! Explicit context for one child operation. Ordinary children inherit none.
//!
//! Applications own the keys and decide which child needs them. The environment
//! is only the transport to that child; the next foundation spawn clears it
//! unless its caller explicitly attaches another context.

use std::{collections::BTreeMap, process::Command, sync::OnceLock};

use crate::Result;

const ENV: &str = "OER_OPERATION_CONTEXT";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    values: BTreeMap<String, String>,
}

impl Context {
    /// The context explicitly attached to this process, parsed once and strictly.
    pub fn current() -> Result<&'static Self> {
        static CURRENT: OnceLock<std::result::Result<Context, String>> = OnceLock::new();
        CURRENT
            .get_or_init(|| match std::env::var(ENV) {
                Ok(text) => serde_json::from_str(&text)
                    .map(|values| Self { values })
                    .map_err(|error| format!("invalid operation context: {error}")),
                Err(std::env::VarError::NotPresent) => Ok(Self::default()),
                Err(error) => Err(format!("invalid operation context: {error}")),
            })
            .as_ref()
            .map_err(|error| error.clone().into())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.set(key, value);
        self
    }

    pub fn remove(&mut self, key: &str) {
        self.values.remove(key);
    }

    /// Attach this context to this command alone, including detached children.
    pub fn apply(&self, command: &mut Command) -> Result<()> {
        command.env(ENV, serde_json::to_string(&self.values)?);
        Ok(())
    }
}

/// Clear implicitly inherited context, preserving an explicitly attached one.
pub(crate) fn prepare(command: &mut Command) {
    if !command.get_envs().any(|(key, _)| key == ENV) {
        command.env_remove(ENV);
    }
}

/// Start building an ordinary command with operation context cleared. This also
/// covers callers using std's spawn for a deliberately detached process.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_remove(ENV);
    command
}
