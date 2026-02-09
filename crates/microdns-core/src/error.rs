use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("database error: {0}")]
    Database(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("zone not found: {0}")]
    ZoneNotFound(String),

    #[error("record not found: {0}")]
    RecordNotFound(String),

    #[error("duplicate zone: {0}")]
    DuplicateZone(String),

    #[error("duplicate record: {0}")]
    DuplicateRecord(String),

    #[error("invalid record data: {0}")]
    InvalidRecord(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// Blanket From impls for redb error types
impl From<redb::Error> for Error {
    fn from(e: redb::Error) -> Self {
        Error::Database(e.to_string())
    }
}

impl From<redb::StorageError> for Error {
    fn from(e: redb::StorageError) -> Self {
        Error::Database(e.to_string())
    }
}

impl From<redb::TableError> for Error {
    fn from(e: redb::TableError) -> Self {
        Error::Database(e.to_string())
    }
}

impl From<redb::TransactionError> for Error {
    fn from(e: redb::TransactionError) -> Self {
        Error::Database(e.to_string())
    }
}

impl From<redb::CommitError> for Error {
    fn from(e: redb::CommitError) -> Self {
        Error::Database(e.to_string())
    }
}

impl From<redb::DatabaseError> for Error {
    fn from(e: redb::DatabaseError) -> Self {
        Error::Database(e.to_string())
    }
}
