use crate::db::error::DbError;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum XBibleError {
    #[error("{error_message}")]
    Generic { error_message: String },
}

impl From<String> for XBibleError {
    fn from(s: String) -> Self {
        XBibleError::Generic { error_message: s }
    }
}

impl From<rusqlite::Error> for XBibleError {
    fn from(e: rusqlite::Error) -> Self {
        XBibleError::Generic {
            error_message: e.to_string(),
        }
    }
}

impl From<DbError> for XBibleError {
    fn from(e: DbError) -> Self {
        XBibleError::Generic {
            error_message: e.to_string(),
        }
    }
}
