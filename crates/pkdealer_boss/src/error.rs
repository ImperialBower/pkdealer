//! Error type for the Boss pipeline.

use std::fmt;

/// Errors surfaced by the `pkdealer_boss` library and CLI.
///
/// Every fallible entry point in this crate — YAML sidecar parsing, session
/// loading, and the eventual CLI — reports failures through this single
/// type rather than leaking library-specific error types (`serde_yaml_bw`'s
/// error, `std::io::Error`, ...) into callers.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::error::BossError;
///
/// let err = BossError::Empty;
/// assert_eq!(err.to_string(), "session contains no attributable hands");
/// ```
#[derive(Debug)]
pub enum BossError {
    /// Reading a session or labels file failed.
    Io(std::io::Error),
    /// A session or labels payload failed to parse.
    Parse(String),
    /// The session contained no attributable hands.
    Empty,
}

impl fmt::Display for BossError {
    /// Formats the error as a one-line, lowercase message prefixed by its
    /// category (`io error: `, `parse error: `), matching the convention
    /// used by `std::error::Error` implementors throughout the workspace.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::error::BossError;
    ///
    /// assert_eq!(
    ///     BossError::Parse("bad yaml".to_string()).to_string(),
    ///     "parse error: bad yaml"
    /// );
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BossError::Io(e) => write!(f, "io error: {e}"),
            BossError::Parse(msg) => write!(f, "parse error: {msg}"),
            BossError::Empty => write!(f, "session contains no attributable hands"),
        }
    }
}

impl std::error::Error for BossError {}

impl From<std::io::Error> for BossError {
    /// Wraps an I/O failure (e.g. reading a session or labels file from
    /// disk) as a [`BossError::Io`], enabling `?` propagation from I/O
    /// code into functions returning `Result<_, BossError>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::error::BossError;
    ///
    /// let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    /// let err: BossError = io_err.into();
    /// assert!(matches!(err, BossError::Io(_)));
    /// ```
    fn from(e: std::io::Error) -> Self {
        BossError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_io_wraps_inner_message() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let err = BossError::Io(io_err);
        assert_eq!(err.to_string(), "io error: missing file");
    }

    #[test]
    fn display_parse_wraps_message() {
        let err = BossError::Parse("bad yaml".to_string());
        assert_eq!(err.to_string(), "parse error: bad yaml");
    }

    #[test]
    fn display_empty_has_fixed_message() {
        let err = BossError::Empty;
        assert_eq!(err.to_string(), "session contains no attributable hands");
    }

    #[test]
    fn from_io_error_constructs_io_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let err: BossError = io_err.into();
        assert!(matches!(err, BossError::Io(_)));
    }

    #[test]
    fn boss_error_implements_std_error() {
        let err = BossError::Empty;
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn debug_format_is_non_empty() {
        let err = BossError::Empty;
        assert!(!format!("{err:?}").is_empty());
    }
}
