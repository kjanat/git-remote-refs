use crate::ObjectId;
use alloc::string::String;
use core::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidUrl,
    InvalidRefName,
    InvalidPrefix,
    InvalidObjectId,
    AuthRequired(u16),
    NotFound,
    Http(u16),
    NotSmartHttp,
    Protocol(&'static str),
    Remote(String),
    UnsupportedObjectFormat,
    UnsupportedFeature(&'static str),
    ChecksumMismatch,
    LimitExceeded(&'static str),
    MissingObject(ObjectId),
}

impl Error {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid-url",
            Self::InvalidRefName => "invalid-ref-name",
            Self::InvalidPrefix => "invalid-prefix",
            Self::InvalidObjectId => "invalid-object-id",
            Self::AuthRequired(_) => "auth-required",
            Self::NotFound => "not-found",
            Self::Http(_) => "http",
            Self::NotSmartHttp => "not-smart-http",
            Self::Protocol(_) => "protocol",
            Self::Remote(_) => "remote",
            Self::UnsupportedObjectFormat => "unsupported-object-format",
            Self::UnsupportedFeature(_) => "unsupported-feature",
            Self::ChecksumMismatch => "checksum-mismatch",
            Self::LimitExceeded(_) => "limit-exceeded",
            Self::MissingObject(_) => "missing-object",
        }
    }

    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::AuthRequired(status) | Self::Http(status) => Some(*status),
            Self::NotFound => Some(404),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => {
                f.write_str("expected an HTTP(S) repository URL without query or fragment")
            }
            Self::InvalidRefName => f.write_str("invalid ref name"),
            Self::InvalidPrefix => f.write_str("invalid ref prefix"),
            Self::InvalidObjectId => f.write_str("expected a full hexadecimal object ID"),
            Self::AuthRequired(status) => write!(
                f,
                "repository requires authentication or does not exist (HTTP {status})"
            ),
            Self::NotFound => f.write_str("repository not found (HTTP 404)"),
            Self::Http(status) => write!(f, "Git server returned HTTP {status}"),
            Self::NotSmartHttp => f.write_str("response is not a Git smart HTTP response"),
            Self::Protocol(message) => write!(f, "Git protocol error: {message}"),
            Self::Remote(message) => write!(f, "Git server error: {message}"),
            Self::UnsupportedObjectFormat => f.write_str("unsupported object format"),
            Self::UnsupportedFeature(feature) => write!(f, "server does not support {feature}"),
            Self::ChecksumMismatch => f.write_str("pack checksum mismatch"),
            Self::LimitExceeded(limit) => write!(f, "limit exceeded: {limit}"),
            Self::MissingObject(oid) => write!(f, "server did not send object {oid}"),
        }
    }
}

impl core::error::Error for Error {}
