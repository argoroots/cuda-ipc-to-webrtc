mod config;
mod pipeline;
mod signaling;
mod viewer;
mod web;

use anyhow::{Context, Result};
use clap::Parser;
use gstreamer::prelude::*;
use log::info;

use config::Config;
use web::StreamInfo;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::parse();
    info!("Starting oden_webrtc_server on port {}", config.port);

    // Exclude Rust-based GStreamer plugins that conflict with our gstreamer-rs linkage
    // (both embed glib type registration, causing "Type already registered" panics).
    // SAFETY: Called before any other threads are spawned (single-threaded at this point).
    unsafe {
        std::env::set_var("GST_PLUGIN_FEATURE_RANK", "rswebrtc:0,fmp4mux:0,isofmp4:0");
    }

    gstreamer::init().context("Failed to initialize GStreamer")?;

    std::thread::spawn(|| {
        let main_loop = gstreamer::glib::MainLoop::new(None, false);
        main_loop.run();
    });

    let addresses = if config.ipc_address.is_empty() {
        vec!["/tmp/ipc_0".to_string()]
    } else {
        config.ipc_address.clone()
    };

    info!("Configuring {} stream(s)", addresses.len());

    let (pipeline, handles, _bus_watch) =
        pipeline::setup_all_streams(&addresses, &config).context("Failed to set up streams")?;

    pipeline
        .set_state(gstreamer::State::Playing)
        .context("Failed to set pipeline to Playing")?;
    info!("Pipeline playing with {} stream(s)", addresses.len());

    let stream_data: Vec<_> = handles
        .into_iter()
        .zip(addresses.iter())
        .enumerate()
        .map(|(i, (handle, addr))| {
            (
                handle,
                StreamInfo {
                    id: i,
                    address: addr.clone(),
                },
            )
        })
        .collect();

    let port = config.port;
    let local = config.local;
    let stun_server = config.stun_server;
    let rt = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;

    rt.block_on(async move {
        let app = web::create_router(stream_data, local, &stun_server);
        let addr = format!("0.0.0.0:{port}");
        info!("HTTP server listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .context("Failed to bind HTTP listener")?;

        axum::serve(listener, app)
            .await
            .context("HTTP server error")
    })?;

    let _ = pipeline.set_state(gstreamer::State::Null);

    Ok(())
}
