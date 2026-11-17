pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// No or a malformed answer to a query (after the retry).
    #[error("{cmd:?}: {problem}")]
    Protocol { cmd: String, problem: String },

    #[error("{what} {value} is out of range ({min} to {max})")]
    OutOfRange {
        what: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },

    #[error("{what} needs hardware revision {needed} or newer; this box has {actual}")]
    HardwareRevision {
        what: &'static str,
        needed: u32,
        actual: u32,
    },

    #[error("{0}")]
    Invalid(String),

    #[error("{0}")]
    NotFound(String),
}
