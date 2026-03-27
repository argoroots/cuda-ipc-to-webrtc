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

## API

| Endpoint | Description |
|----------|-------------|
| `GET /streams` | Returns JSON list of available streams with id and name |
| `WS /ws/{id}` | WebSocket endpoint for WebRTC signaling (SDP offer/answer + ICE candidates) |

