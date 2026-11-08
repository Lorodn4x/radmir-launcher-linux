use serde::{Serialize, Serializer};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),
    #[error("{message}")]
    Coded { code: String, message: String },
    #[error("{message}. Журнал {log_path}")]
    Diagnostic {
        code: String,
        message: String,
        log_path: String,
    },
    #[error("Ошибка файловой операции: {0}")]
    Io(#[from] std::io::Error),
    #[error("Ошибка сети: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Некорректный JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Ошибка локальной базы данных: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Некорректный URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("Некорректный XML: {0}")]
    Xml(#[from] quick_xml::DeError),
    #[error("Фоновая задача прервана: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl AppError {
    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Coded {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn code(&self) -> &str {
        match self {
            Self::Coded { code, .. } | Self::Diagnostic { code, .. } => code,
            Self::Network(_) => "network_error",
            Self::Io(_) => "filesystem_error",
            _ => "operation_failed",
        }
    }
    pub fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
    pub fn with_log(
        code: impl Into<String>,
        message: impl Into<String>,
        path: &std::path::Path,
    ) -> Self {
        Self::Diagnostic {
            code: code.into(),
            message: message.into(),
            log_path: path.to_string_lossy().into(),
        }
    }
    pub fn log_path(&self) -> Option<&str> {
        match self {
            Self::Diagnostic { log_path, .. } => Some(log_path),
            _ => None,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde::Serialize::serialize(
            &serde_json::json!({"code": self.code(), "message": match self { Self::Diagnostic { message, .. } => message.clone(), _ => self.to_string() }, "retryable": self.code() != "invalid_settings", "log_path": self.log_path()}),
            serializer,
        )
    }
}
