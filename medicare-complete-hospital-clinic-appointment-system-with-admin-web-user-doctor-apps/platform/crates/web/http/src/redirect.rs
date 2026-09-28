//! Web-flow redirect primitives (S9): typed `Redirect` with the standard
//! 302/303/307 variants, rendered as a `Location` response at the edge.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// A redirect response: status code + Location header.
#[derive(Debug, Clone)]
pub struct Redirect {
    pub status: StatusCode,
    pub location: String,
}

impl Redirect {
    /// 302 Found — the historical default for post-action redirects.
    pub fn to(location: impl Into<String>) -> Self {
        Self::with_status(StatusCode::FOUND, location)
    }

    /// 303 See Other — post/redirect/get: the follow-up must be a GET.
    pub fn see_other(location: impl Into<String>) -> Self {
        Self::with_status(StatusCode::SEE_OTHER, location)
    }

    /// 307 Temporary Redirect — preserves the original method.
    pub fn temporary(location: impl Into<String>) -> Self {
        Self::with_status(StatusCode::TEMPORARY_REDIRECT, location)
    }

    pub fn with_status(status: StatusCode, location: impl Into<String>) -> Self {
        Self {
            status,
            location: location.into(),
        }
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        (self.status, [(header::LOCATION, self.location.clone())]).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(redirect: Redirect) -> Response {
        redirect.into_response()
    }

    #[test]
    fn found_redirect_sets_location() {
        let resp = render(Redirect::to("/dashboard"));
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/dashboard");
    }

    #[test]
    fn see_other_uses_303() {
        assert_eq!(
            render(Redirect::see_other("/x")).status(),
            StatusCode::SEE_OTHER
        );
    }

    #[test]
    fn temporary_uses_307() {
        assert_eq!(
            render(Redirect::temporary("/x")).status(),
            StatusCode::TEMPORARY_REDIRECT
        );
    }
}
