use crate::db::error::DbError;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FreeBibleError {
    #[error("{error_message}")]
    Generic { error_message: String },
}

impl From<String> for FreeBibleError {
    fn from(s: String) -> Self {
        FreeBibleError::Generic { error_message: s }
    }
}

impl From<rusqlite::Error> for FreeBibleError {
    fn from(e: rusqlite::Error) -> Self {
        FreeBibleError::Generic { error_message: e.to_string() }
    }
}

impl From<DbError> for FreeBibleError {
    fn from(e: DbError) -> Self {
        FreeBibleError::Generic {
            error_message: e.to_string(),
        }
    }
}