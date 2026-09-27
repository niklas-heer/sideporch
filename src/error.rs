use std::fmt::Display;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::views;

/// Errors a request handler can return. Each maps to one HTTP status.
#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Forbidden,
    NotFound,
    Gone(String),
    Internal(String),
}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message) | Self::Gone(message) | Self::Internal(message) => {
                f.write_str(message)
            }
            Self::Forbidden => f.write_str("forbidden"),
            Self::NotFound => f.write_str("not found"),
        }
    }
}

impl std::error::Error for AppError {}

impl AppError {
    pub fn internal(error: impl Display) -> Self {
        Self::Internal(error.to_string())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        Self::internal(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "You don't have access to this page.".to_owned(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "This page doesn't exist, or it was removed.".to_owned(),
            ),
            Self::Gone(message) => (StatusCode::GONE, message),
            Self::Internal(detail) => {
                tracing::error!(%detail, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong on the server. Try again in a moment.".to_owned(),
                )
            }
        };
        (status, views::error_page(status, &message)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
