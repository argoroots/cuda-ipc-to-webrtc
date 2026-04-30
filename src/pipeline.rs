use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use gstreamer_sdp as gst_sdp;
use gstreamer_webrtc as gst_webrtc;
use log::{debug, error, info, warn};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::signaling::SignalingMessage;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct SessionGuard {
    pipeline: gst::Pipeline,
    tee_pad: gst::Pad,
    queue: gst::Element,
    webrtcbin: gst::Element,
    stream_index: usize,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        info!("Stream {}: cleaning up WebRTC session", self.stream_index);
        let _ = self.queue.set_state(gst::State::Null);
        let _ = self.webrtcbin.set_state(gst::State::Null);

        if let Some(queue_sink) = self.queue.static_pad("sink") {
            let _ = self.tee_pad.unlink(&queue_sink);
        }
        if let Some(tee) = self.tee_pad.parent_element() {
            tee.release_request_pad(&self.tee_pad);
        }
        let _ = self.pipeline.remove(&self.queue);
        let _ = self.pipeline.remove(&self.webrtcbin);
    }
}

#[derive(Clone)]
pub struct StreamHandle {
    pipeline: gst::Pipeline,
    tee: gst::Element,
    stun_server: String,
    stream_index: usize,
}

impl StreamHandle {
    pub fn new_session(
        &self,
    ) -> Result<(
        mpsc::UnboundedSender<SignalingMessage>,
        mpsc::UnboundedReceiver<SignalingMessage>,
        SessionGuard,
    )> {
        let idx = self.stream_index;
        let session_id = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);

        let (to_pipeline_tx, to_pipeline_rx) = mpsc::unbounded_channel::<SignalingMessage>();
        let (from_pipeline_tx, from_pipeline_rx) = mpsc::unbounded_channel::<SignalingMessage>();

        let queue = gst::ElementFactory::make("queue")
            .build()
            .expect("Failed to create queue");

        let webrtcbin = gst::ElementFactory::make("webrtcbin")
            .name(&format!("webrtc-{idx}-{session_id}"))
            .property_from_str("bundle-policy", "max-bundle")
            .property("stun-server", &self.stun_server)
            .build()
            .expect("Failed to create webrtcbin");

        self.pipeline
            .add_many([&queue, &webrtcbin])
            .expect("Failed to add elements to pipeline");

        let tee_pad = self
            .tee
            .request_pad_simple("src_%u")
            .expect("Failed to request tee pad");
        let queue_sink = queue
            .static_pad("sink")
            .expect("queue element missing 'sink' pad");
        tee_pad
            .link(&queue_sink)
            .expect("Failed to link tee to queue");
        queue
            .link(&webrtcbin)
            .expect("Failed to link queue to webrtcbin");

        let tx = from_pipeline_tx.clone();
        webrtcbin.connect("on-ice-candidate", false, move |args| {
            let sdp_m_line_index = args[1].get::<u32>().expect("mlineindex is not u32");
            let candidate = args[2].get::<String>().expect("candidate is not string");

            if tx
                .send(SignalingMessage::Ice {
                    candidate,
                    sdp_m_line_index,
                })
                .is_err()
            {
                warn!("Failed to forward ICE candidate — receiver dropped");
            }
            None
        });

        queue
            .sync_state_with_parent()
            .expect("Failed to sync queue state");
        webrtcbin
            .sync_state_with_parent()
            .expect("Failed to sync webrtcbin state");

        let webrtc_offer = webrtcbin.clone();
        let tx = from_pipeline_tx;
        let promise = gst::Promise::with_change_func(move |reply| {
            let reply = match reply {
                Ok(Some(r)) => r,
                Ok(None) => {
                    error!("Stream {idx}: create offer returned no reply");
                    return;
                }
                Err(_) => {
                    error!("Stream {idx}: create offer promise interrupted");
                    return;
                }
            };

            let offer = reply
                .value("offer")
                .expect("reply has no offer")
                .get::<gst_webrtc::WebRTCSessionDescription>()
                .expect("offer is not a WebRTCSessionDescription");

            let sdp_text = offer.sdp().as_text().expect("SDP to_string failed");
            info!("Stream {idx}: created SDP offer");

            let set_promise = gst::Promise::with_change_func(|_| {});
            webrtc_offer.emit_by_name::<()>("set-local-description", &[&offer, &set_promise]);

            if tx.send(SignalingMessage::Offer { sdp: sdp_text }).is_err() {
                warn!("Stream {idx}: failed to send SDP offer — receiver dropped");
            }
        });

        webrtcbin.emit_by_name::<()>("create-offer", &[&None::<gst::Structure>, &promise]);

        let webrtc = webrtcbin.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let mut rx = to_pipeline_rx;
            while let Some(msg) = rx.recv().await {
                match msg {
                    SignalingMessage::Answer { sdp } => {
                        info!("Stream {idx}: received SDP answer");
                        let sdp = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
                            .expect("Failed to parse SDP answer");
                        let answer = gst_webrtc::WebRTCSessionDescription::new(
                            gst_webrtc::WebRTCSDPType::Answer,
                            sdp,
                        );
                        let promise = gst::Promise::with_change_func(|_| {});
                        webrtc.emit_by_name::<()>("set-remote-description", &[&answer, &promise]);
                    }
                    SignalingMessage::Ice {
                        candidate,
                        sdp_m_line_index,
                    } => {
                        webrtc.emit_by_name::<()>(
                            "add-ice-candidate",
                            &[&sdp_m_line_index, &candidate],
                        );
                    }
                    SignalingMessage::Offer { .. } => {}
                }
            }
        });

        let guard = SessionGuard {
            pipeline: self.pipeline.clone(),
            tee_pad,
            queue,
            webrtcbin,
            stream_index: idx,
        };

        Ok((to_pipeline_tx, from_pipeline_rx, guard))
    }
}

pub fn setup_all_streams(
    addresses: &[String],
    config: &Config,
) -> Result<(gst::Pipeline, Vec<StreamHandle>, gst::bus::BusWatchGuard)> {
    let encoder_name = if gst::ElementFactory::find("nvcudah264enc").is_some() {
        info!("Using nvcudah264enc (GPU encoder)");
        "nvcudah264enc"
    } else {
        info!("nvcudah264enc not available, falling back to x264enc (CPU encoder)");
        "x264enc"
    };

    let has_relay = !config.relay_url.is_empty();

    let mut chains = Vec::new();
    for (i, addr) in addresses.iter().enumerate() {
        let source = if config.test_src {
            format!(
                "videotestsrc is-live=true pattern={pattern} ! video/x-raw,format=NV12,width=640,height=360,framerate=30/1",
                pattern = i % 25,
            )
        } else {
            format!(
                "cudaipcsrc address={addr} ! \
                 cudadownload ! videorate drop-only=true ! video/x-raw,framerate=30/1 ! \
                 cudaupload"
            )
        };

        let encoder = if encoder_name == "nvcudah264enc" {
            format!(
                "nvcudah264enc bitrate={bitrate} preset=p1 tune=ultra-low-latency rate-control=cbr ! video/x-h264,profile=baseline",
                bitrate = config.bitrate,
            )
        } else {
            format!(
                "x264enc bitrate={bitrate} speed-preset=ultrafast tune=zerolatency ! video/x-h264,profile=baseline",
                bitrate = config.bitrate,
            )
        };

        let relay_url = config.relay_url.get(i);

        let chain = if has_relay {
            // When relay is enabled, tee raw H.264 before RTP packetization:
            //   encoder → tee (raw)
            //     ├─ queue → rtph264pay → tee (RTP, for local WebRTC viewers)
            //     └─ queue → h264parse → flvmux → rtmpsink (RTMP push to MediaMTX)
            let rtmp_branch = if let Some(url) = relay_url {
                format!(
                    " raw_t{i}. ! queue ! h264parse ! flvmux streamable=true ! rtmpsink location={url}",
                )
            } else {
                String::new()
            };

            format!(
                "{source} ! {encoder} ! tee name=raw_t{i} allow-not-linked=true \
                 raw_t{i}. ! queue ! rtph264pay config-interval=-1 pt=96 ! \
                 application/x-rtp,media=video,encoding-name=H264,payload=96 ! \
                 tee name=t{i} allow-not-linked=true\
                 {rtmp_branch}"
            )
        } else {
            // Original pipeline: no relay, tee after RTP packetization
            format!(
                "{source} ! {encoder} ! rtph264pay config-interval=-1 pt=96 ! \
                 application/x-rtp,media=video,encoding-name=H264,payload=96 ! \
                 tee name=t{i} allow-not-linked=true"
            )
        };

        debug!("Stream {i}: {chain}");
        chains.push(chain);
    }

    let pipeline_str = chains.join(" ");

    let pipeline = gst::parse::launch(&pipeline_str)
        .context("Failed to parse multi-stream pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("Pipeline element is not a Pipeline"))?;

    let bus = pipeline.bus().expect("pipeline has no bus");
    let bus_watch = bus
        .add_watch(move |_, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    let src = err.src().map(|s| s.name().to_string()).unwrap_or_default();
                    error!("Bus error from {src}: {}", err.error());
                    if let Some(debug) = err.debug() {
                        error!("Bus debug: {debug}");
                    }
                }
                MessageView::Warning(warn) => {
                    let src = warn.src().map(|s| s.name().to_string()).unwrap_or_default();
                    info!("Bus warning from {src}: {}", warn.error());
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })
        .expect("Failed to add bus watch");

    let mut handles = Vec::new();
    for i in 0..addresses.len() {
        let tee_name = format!("t{i}");
        let tee = pipeline
            .by_name(&tee_name)
            .with_context(|| format!("No element named '{tee_name}' in pipeline"))?;

        handles.push(StreamHandle {
            pipeline: pipeline.clone(),
            tee,
            stun_server: config.stun_server.clone(),
            stream_index: i,
        });
    }

    if has_relay {
        for (i, url) in config.relay_url.iter().enumerate() {
            info!("Stream {i}: RTMP relay → {url}");
        }
    }

    Ok((pipeline, handles, bus_watch))
}
