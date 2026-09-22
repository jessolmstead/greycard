use std::fmt;

/// Errors from decoding and developing.
#[derive(Debug)]
pub enum Error {
    /// The decoder backend could not read the file.
    Decode(String),
    /// The file decoded but describes something the engine does not handle.
    Unsupported(String),
    /// The file carries no usable color calibration.
    NoCalibration,
    /// A camera profile file could not be read or used.
    Profile(String),
    /// A lookup table file could not be read or used.
    Lut(String),
    /// An output file could not be encoded.
    Encode(String),
    /// A registration could not be fitted: no structure in the frames,
    /// or too little of them in common.
    NoFit(String),
    /// A color computation failed (degenerate white, singular matrix, ...).
    Color(rawcolor::ColorError),
    Io(std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Decode(msg) => write!(f, "decode failed: {msg}"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::NoCalibration => write!(f, "file carries no usable 3x3 color calibration"),
            Error::Profile(msg) => write!(f, "camera profile: {msg}"),
            Error::Lut(msg) => write!(f, "lookup table: {msg}"),
            Error::Encode(msg) => write!(f, "encode failed: {msg}"),
            Error::NoFit(why) => write!(f, "no fit: {why}"),
            Error::Color(e) => write!(f, "color error: {e}"),
            Error::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Color(e) => Some(e),
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rawcolor::ColorError> for Error {
    fn from(e: rawcolor::ColorError) -> Self {
        Error::Color(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
