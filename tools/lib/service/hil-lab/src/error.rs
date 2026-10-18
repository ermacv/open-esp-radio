//! A laboratory operation failed independently of the target's assertions.

#[derive(Debug)]
pub struct Error {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Mark a failed laboratory operation while retaining cancellation and I/O causes.
    pub fn context(
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        if source.is::<Self>() {
            return source;
        }
        Box::new(Self {
            message: source.to_string(),
            source: Some(source),
        })
    }

    /// SSH uses 255 for transport/authentication failure. Other exit codes
    /// belong to the remote command, which may intentionally query absence.
    pub fn ssh_output(output: std::process::Output) -> crate::Result<std::process::Output> {
        if output.status.code() == Some(255) {
            return Err(Self::new(format!(
                "SSH transport failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        Ok(output)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}
