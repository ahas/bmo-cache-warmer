//! A refused request (docs/api.md, "Wire format" and "Codes"): a status and a body of a kebab-case `code`
//! and an English `error`, and the extractors that refuse a path, a query or a body in that shape.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::store::StoreError;
use crate::wire::WireError;
use crate::words::is_name;

/// A refusal: its status, its code and its sentence.
#[derive(Debug)]
pub struct Refusal {
  status: StatusCode,
  code: &'static str,
  error: String,
}

impl Refusal {
  pub fn new(status: StatusCode, code: &'static str, error: impl Into<String>) -> Self {
    Self {
      status,
      code,
      error: error.into(),
    }
  }

  pub fn bad_request(error: impl Into<String>) -> Self {
    Self::new(StatusCode::BAD_REQUEST, "bad-request", error)
  }

  /// A rule of the service refusing a well-formed request, the code naming the rule.
  pub fn rule(code: &'static str, error: impl Into<String>) -> Self {
    Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, error)
  }

  pub fn invalid_clock(error: impl Into<String>) -> Self {
    Self::rule("invalid-clock", error)
  }

  pub fn conflict(error: impl Into<String>) -> Self {
    Self::new(StatusCode::CONFLICT, "conflict", error)
  }
}

impl IntoResponse for Refusal {
  fn into_response(self) -> Response {
    let body = WireError {
      code: self.code.to_owned(),
      error: self.error,
    };

    (self.status, Json(body)).into_response()
  }
}

impl From<StoreError> for Refusal {
  /// The store's refusals at their own status.
  fn from(error: StoreError) -> Self {
    let sentence = error.to_string();
    match error {
      StoreError::UnknownConsumer | StoreError::UnknownTarget | StoreError::UnknownGroup => {
        Self::new(StatusCode::NOT_FOUND, "not-found", sentence)
      }
      StoreError::GroupInUse => Self::conflict(sentence),
      StoreError::Database(_) => {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "failed", "The store could not be read or written.")
      }
    }
  }
}

/// Refuses a name out of its shape, saying which field held it.
pub fn checked_name(field: &str, name: &str) -> Result<(), Refusal> {
  if is_name(name) {
    return Ok(());
  }
  Err(Refusal::bad_request(format!("The {field} must be a name of 1 to 256 bytes.")))
}

/// The names in a request's path, each checked for its shape.
pub struct Names<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Names<T> {
  type Rejection = Refusal;

  async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Refusal> {
    let unread = |_| Refusal::bad_request("A name in the path does not read as UTF-8.");
    let Path(pairs) =
      Path::<Vec<(String, String)>>::from_request_parts(parts, state).await.map_err(unread)?;
    for (field, name) in &pairs {
      checked_name(field, name)?;
    }
    let Path(names) = Path::<T>::from_request_parts(parts, state).await.map_err(unread)?;

    Ok(Self(names))
  }
}

/// A query string read into `T`.
pub struct Narrowed<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Narrowed<T> {
  type Rejection = Refusal;

  async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Refusal> {
    let Query(query) = Query::<T>::from_request_parts(parts, state)
      .await
      .map_err(|error| Refusal::bad_request(format!("The query does not read: {}.", error.body_text())))?;

    Ok(Self(query))
  }
}

/// A JSON body read into `T`.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
  type Rejection = Refusal;

  async fn from_request(request: Request, state: &S) -> Result<Self, Refusal> {
    let bytes = Bytes::from_request(request, state).await.map_err(|error| {
      Refusal::bad_request(format!("The body could not be read: {}.", error.body_text()))
    })?;
    // A body left out reads as an empty object, so only a route whose every field may be absent takes it.
    let text: &[u8] = if bytes.is_empty() { b"{}" } else { &bytes };
    let body = serde_json::from_slice(text)
      .map_err(|error| Refusal::bad_request(format!("The body does not read as asked: {error}.")))?;

    Ok(Self(body))
  }
}
