use std::sync::Arc;

use axum::{
    extract::{self, OriginalUri},
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse, Response},
    Extension,
};
use foundations::telemetry::log;

#[derive(Debug, Clone)]
pub(crate) enum AppError {
    // this is an Arc so that it's clonable for use in the Extension
    AnyError(Arc<anyhow::Error>),
    TemplateError(Arc<minijinja::Error>),
    Unauthorized,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let self_arc = Extension(Arc::new(self.clone()));

        match self {
            Self::AnyError(e) => {
                log::error!("{:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    self_arc,
                    "Something went wrong.",
                )
            }
            Self::Unauthorized => {
                log::warn!("Unauthorized!");
                (
                    StatusCode::UNAUTHORIZED,
                    self_arc,
                    "The secret is invalid or expired",
                )
            }
            Self::TemplateError(err) => {
                log::warn!("Could not evaluate template"; "err" => err.to_string());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    self_arc,
                    "Something went wrong",
                )
            }
        }
        .into_response()
    }
}

impl From<minijinja::Error> for AppError {
    fn from(err: minijinja::Error) -> Self {
        Self::TemplateError(Arc::new(err))
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self::AnyError(Arc::new(err))
    }
}

pub async fn error_middleware(
    extract::State(state): extract::State<Arc<minijinja::Environment<'_>>>,
    req: extract::Request,
    next: middleware::Next,
) -> Response {
    let original_uri = req
        .extensions()
        .get::<OriginalUri>()
        .map(|p| p.0.path().to_owned());

    log::debug!("error middleware"; "original_uri" => format!("{:?}", original_uri));

    let resp = next.run(req).await;

    if let Some(failure) = resp.extensions().get::<Arc<AppError>>() {
        match handle_error(failure, state, original_uri, &resp) {
            Ok(resp) => resp,
            Err(err) => {
                log::warn!("error handler failed"; "err"=> format!("{:?}", err));
                err.into_response()
            }
        }
    } else {
        log::debug!("no failure");
        resp
    }
}

fn handle_error(
    failure: &Arc<AppError>,
    state: Arc<minijinja::Environment>,
    original_uri: Option<String>,
    resp: &axum::http::Response<axum::body::Body>,
) -> Result<axum::http::Response<axum::body::Body>, AppError> {
    let template = match failure.as_ref() {
        AppError::Unauthorized => "_unauthorized.html.j2",
        AppError::AnyError(_err) => "_any_error.html.j2",
        AppError::TemplateError(_err) => "_any_error.html.j2",
    };

    let status = resp.status();

    if let Ok(tmpl) = state.get_template(template) {
        let context = minijinja::context!(status_code => status.as_u16(), uri => original_uri);

        Ok((status, Html(tmpl.render(context)?)).into_response())
    } else {
        log::warn!("Cannot render template"; "template" => template);
        Ok(<AppError as Clone>::clone(failure).into_response())
    }
}
