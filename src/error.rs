use std::fmt;

/// An error with a human-readable message, suitable for showing to the user.
///
/// Like `anyhow::Error`, it converts from any standard error and deliberately
/// does not implement [`std::error::Error`] itself, so `?` works everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Error(msg.into())
    }

    /// Prefix the message with context, e.g. the file being processed.
    pub fn context(self, ctx: impl fmt::Display) -> Self {
        Error(format!("{ctx}: {}", self.0))
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: std::error::Error> From<E> for Error {
    fn from(e: E) -> Self {
        Error(e.to_string())
    }
}

/// Attach context to any fallible result.
pub trait Context<T> {
    fn context(self, ctx: impl fmt::Display) -> Result<T>;
}

impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context(self, ctx: impl fmt::Display) -> Result<T> {
        self.map_err(|e| e.into().context(ctx))
    }
}

impl<T> Context<T> for Option<T> {
    fn context(self, ctx: impl fmt::Display) -> Result<T> {
        self.ok_or_else(|| Error::new(ctx.to_string()))
    }
}
