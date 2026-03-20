use axum::response::Html;

pub async fn approvals_ui_handler() -> Html<&'static str> {
    Html(include_str!("approvals.html"))
}
