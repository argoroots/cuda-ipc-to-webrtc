# Oden WebRTC Server

A Rust-based WebRTC streaming server that captures video from CUDA IPC sources and streams them to web browsers in real-time using H.264 encoding.

## Features

- **Multi-stream support** — stream multiple CUDA IPC sources simultaneously from a single instance
- **Automatic codec selection** — GPU encoding via `nvcudah264enc` with automatic CPU (`x264enc`) fallback
- **Low-latency** — H.264 baseline profile, CBR rate control, ultrafast presets
- **Built-in viewer** — embedded HTML5 viewer for quick testing (behind `--local` flag)
- **Dynamic sessions** — WebRTC sessions created on-demand with automatic RAII cleanup

## Prerequisites

- Rust (edition 2024)
- GStreamer 1.0 development libraries
- GStreamer plugins: `gstreamer-plugins-base`, `gstreamer-plugins-bad` (for WebRTC)
- Optional: NVIDIA CUDA libraries and GStreamer CUDA plugins for GPU encoding

## Build

```bash
cargo build --release
```

## Usage

**Test mode** (synthetic video source with built-in viewer):

```bash
./target/release/oden_webrtc_server --test-src --local
```

Then open http://localhost:8080.

**Production** (streaming from CUDA IPC):

```bash
./target/release/oden_webrtc_server \
  --ipc-address /tmp/ipc_0 \
  --port 8080 \
  --bitrate 5000
```

**Multiple streams:**

```bash
./target/release/oden_webrtc_server \
  --ipc-address /tmp/ipc_0 \
  --ipc-address /tmp/ipc_1 \
  --ipc-address /tmp/ipc_2
```

**Relay to MediaMTX** (push streams to a remote server via RTMP):

```bash
./target/release/oden_webrtc_server \
  --ipc-address /tmp/ipc_0 \
  --relay-url rtmp://mediamtx.example.com/stream-0
```

**Multiple streams with relay:**

```bash
./target/release/oden_webrtc_server \
  --ipc-address /tmp/ipc_0 \
  --ipc-address /tmp/ipc_1 \
  --relay-url rtmp://mediamtx.example.com/stream-0 \
  --relay-url rtmp://mediamtx.example.com/stream-1
```

**Enable debug logging:**

```bash
RUST_LOG=debug ./target/release/oden_webrtc_server --test-src --local
```

## Configuration

| Option | Default | Description |
|--------|---------|-------------|
| `--ipc-address` | `/tmp/ipc_0` | CUDA IPC socket path(s), can be specified multiple times |
| `--port` | `8080` | HTTP server listen port |
| `--bitrate` | `4000` | Video bitrate in kbps |
| `--test-src` | `false` | Use synthetic test video instead of CUDA IPC |
| `--stun-server` | `stun://stun.l.google.com:19302` | STUN server URL for NAT traversal |
| `--local` | `false` | Serve embedded HTML viewer at `/` |
| `--relay-url` | _(none)_ | RTMP URL(s) to push streams to (e.g. MediaMTX), can be specified multiple times |

## API

| Endpoint | Description |
|----------|-------------|
| `GET /streams` | Returns JSON list of available streams with id and name |
| `WS /ws/{id}` | WebSocket endpoint for WebRTC signaling (SDP offer/answer + ICE candidates) |

## Remote Streaming via MediaMTX

For making streams accessible to multiple remote users, this server can push RTMP to a [MediaMTX](https://github.com/bluenviron/mediamtx) relay deployed on Azure Kubernetes (or any server).

### Architecture

```
Local PC (oden_webrtc_server)  ──RTMP──▶  MediaMTX (Azure K8s)  ──HLS/WHEP──▶  Viewers
```

When `--relay-url` is provided, the GStreamer pipeline tees the encoded H.264 before RTP packetization and pushes an RTMP stream to MediaMTX. Local WebRTC viewers continue to work as before.

### Viewer URLs

MediaMTX automatically serves each ingested stream over multiple protocols:

| Protocol | URL | Latency | Notes |
|----------|-----|---------|-------|
| HLS | `https://streams.yourdomain.com/stream-0/index.m3u8` | 2-6s | Works in `<video>` tags (Safari native, Chrome/Firefox via hls.js) |
| LL-HLS | Same URL (configured server-side) | 1-3s | Low-latency HLS variant |
| WHEP | `https://streams.yourdomain.com/stream-0/whep` | ~200ms | WebRTC playback, needs ~20 lines of JS |

### Embedding HLS in a webpage

```html
<script src="https://cdn.jsdelivr.net/npm/hls.js@latest"></script>
<video id="v" autoplay muted></video>
<script>
  const v = document.getElementById('v');
  if (v.canPlayType('application/vnd.apple.mpegurl')) {
    v.src = 'https://streams.yourdomain.com/stream-0/index.m3u8';
  } else {
    const hls = new Hls();
    hls.loadSource('https://streams.yourdomain.com/stream-0/index.m3u8');
    hls.attachMedia(v);
  }
</script>
```

### Kubernetes Deployment

Kubernetes manifests for MediaMTX are in `k8s/mediamtx/`, designed for ArgoCD sync. See `PLAN.md` for full deployment details.

### Testing Locally

```bash
# Start MediaMTX
docker run --rm -p 8888:8888 -p 1935:1935 bluenviron/mediamtx

# Start the server with RTMP relay
cargo run -- --test-src --local --relay-url rtmp://localhost/stream-0

# View via HLS
open http://localhost:8888/stream-0/
```

