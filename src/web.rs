use std::sync::Arc;

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Html,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::pipeline::{SessionGuard, StreamHandle};
use crate::signaling::SignalingMessage;
use crate::viewer::viewer_html;

#[derive(Clone, Serialize)]
pub struct StreamInfo {
    pub id: usize,
    pub address: String,
}

#[derive(Clone)]
struct AppState {
    streams: Arc<Vec<StreamHandle>>,
    stream_info: Arc<Vec<StreamInfo>>,
}

pub fn create_router(
    streams: Vec<(StreamHandle, StreamInfo)>,
    local: bool,
    stun_server: &str,
) -> Router {
    let mut handles = Vec::new();
    let mut info = Vec::new();

    for (h, si) in streams {
        handles.push(h);
        info.push(si);
    }

    let state = AppState {
        streams: Arc::new(handles),
        stream_info: Arc::new(info),
    };

    let mut router = Router::new();

    if local {
        let html = viewer_html(stun_server);
        router = router.route("/", get(|| async { Html(html) }));
    }

    router
        .route("/streams", get(list_streams))
        .route("/ws/{id}", get(ws_handler))
        .with_state(state)
}

async fn list_streams(State(state): State<AppState>) -> axum::response::Json<Vec<StreamInfo>> {
    axum::response::Json((*state.stream_info).clone())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<usize>,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Some(handle) = state.streams.get(id) {
            match handle.new_session() {
                Ok((to_pipeline, from_pipeline, guard)) => {
                    handle_websocket(socket, to_pipeline, from_pipeline, guard, id).await;
                }
                Err(e) => {
                    error!("Stream {id}: failed to create session: {e}");
                }
            }
        } else {
            error!("Stream {id}: not found");
        }
    })
}

async fn handle_websocket(
    socket: WebSocket,
    to_pipeline: mpsc::UnboundedSender<SignalingMessage>,
    mut from_pipeline: mpsc::UnboundedReceiver<SignalingMessage>,
    _guard: SessionGuard,
    id: usize,
) {
    info!("Stream {id}: WebSocket peer connected");

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Forward pipeline -> browser
    let send_task = tokio::spawn(async move {
        while let Some(msg) = from_pipeline.recv().await {
            let json = serde_json::to_string(&msg).expect("Failed to serialize signaling message");
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Forward browser -> pipeline
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => match serde_json::from_str::<SignalingMessage>(&text) {
                    Ok(signaling_msg) => {
                        if to_pipeline.send(signaling_msg).is_err() {
                            break;
                        }
                    }
                    Err(e) => error!("Stream {id}: failed to parse signaling message: {e}"),
                },
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    let (mut send_task, mut recv_task) = (send_task, recv_task);
    tokio::select! {
        _ = &mut send_task => {}
        _ = &mut recv_task => {}
    }
    send_task.abort();
    recv_task.abort();

    info!("Stream {id}: WebSocket peer disconnected");
    // _guard dropped here → pipeline set to Null
}
