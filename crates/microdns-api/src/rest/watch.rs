use crate::{AppState, DashboardEvent};
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

pub fn router() -> Router<AppState> {
    Router::new().route("/watch", get(watch_handler))
}

#[derive(Deserialize)]
struct WatchParams {
    /// Comma-separated list of event types to filter.
    /// e.g. "dhcp,dns,zones,records,leases"
    /// If empty, all events are streamed.
    #[serde(default)]
    types: Option<String>,
}

fn matches_filter(event: &DashboardEvent, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let category = match event {
        DashboardEvent::DhcpPoolChanged { .. } => "dhcp",
        DashboardEvent::DhcpReservationChanged { .. } => "dhcp",
        DashboardEvent::DnsForwarderChanged { .. } => "dns",
        DashboardEvent::LeaseChanged { .. } => "leases",
        DashboardEvent::ZoneChanged { .. } => "zones",
        DashboardEvent::RecordChanged { .. } => "records",
    };
    filters.iter().any(|f| f == category)
}

async fn watch_handler(
    State(state): State<AppState>,
    Query(params): Query<WatchParams>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let filters: Vec<String> = params
        .types
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        match result {
            Ok(event) => {
                if matches_filter(&event, &filters) {
                    if let Ok(json) = serde_json::to_string(&event) {
                        let sse_event = Event::default().data(json);
                        Some(Ok(sse_event))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Err(_) => None, // Lagged or closed
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
