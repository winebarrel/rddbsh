//! Engine error model (port of include/ddbsh/error.hpp / result.hpp).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errc {
    ParseError,
    NoSuchTable,
    TableAlreadyExists,
    ConditionalCheckFailed,
    ServiceError,
    InvalidArgument,
    Cancelled,
    NotReducibleToGetitem,
    NotImplemented,
    UnboundParameter,
    InternalError,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub code: Errc,
    pub message: String,
    pub service_error_name: Option<String>,
    pub request_id: Option<String>,
}

impl Error {
    pub fn new(code: Errc, message: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
            service_error_name: None,
            request_id: None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Shorthand for `Err(Error::new(code, message))`.
pub fn fail<T>(code: Errc, message: impl Into<String>) -> Result<T> {
    Err(Error::new(code, message))
}
