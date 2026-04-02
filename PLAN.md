# Plan: Stream CUDA IPC Video to Azure for Multi-User Access

## Context

The local PC runs `oden_webrtc_server` capturing CUDA IPC video and streaming via WebRTC to local browsers. The goal is to make these streams accessible to 10-50 concurrent viewers via a website hosted through Azure Kubernetes, where users just need a URL to embed in `<video>` tags.

## Recommendation: MediaMTX as the Cloud Relay

After evaluating LiveKit, Janus, MediaMTX, and building a relay in-project:

- **MediaMTX wins** — it's a single Go binary (~15MB) that accepts RTMP/WHIP input and serves **HLS** (works in `<video>` tags), **WHEP** (low-latency WebRTC), RTSP, and more. Zero dependencies, trivial K8s deployment.
- **LiveKit** is overkill — requires Redis, multiple services, designed for interactive conferencing, and its primary viewer path needs a JS SDK (not `<video>` tag friendly).
- **Janus** — no HLS, no WHIP, complex C plugin architecture, requires JS library.
- **Building in-project** — would mean reimplementing HLS segmentation, playlist management, multi-protocol serving. Not worth it.

## Architecture

```
┌─────────────────────┐         RTMP push          ┌──────────────────────┐
│  Local PC            │  ──────────────────────▶  │  Azure K8s (MediaMTX) │
│  oden_webrtc_server  │   rtmp://relay/stream-0   │                      │
│  (CUDA IPC → H.264)  │                           │  HLS:  /stream-0/    │
│                      │                           │  WHEP: /stream-0/whep│
└─────────────────────┘                           └──────────────────────┘
                                                          │
                                                     ┌────┴────┐
                                                     │ Viewers  │
                                                     │ <video>  │
                                                     └─────────┘
```

## Stream URLs for Embedding

**HLS** (2-6s latency, works in `<video>` tags):
```
https://streams.yourdomain.com/stream-0/index.m3u8
```

**LL-HLS** (~1-3s latency, same URL, configured server-side)

**WHEP** (~200ms latency, needs ~20 lines of JS):
```
https://streams.yourdomain.com/stream-0/whep
```

HLS embedding example:
```html
<!-- Safari: native -->
<video src="https://streams.yourdomain.com/stream-0/index.m3u8" autoplay muted></video>

<!-- Chrome/Firefox: needs hls.js (one script tag) -->
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

---

## Implementation Steps

### Step 1: Restructure GStreamer Pipeline (`src/pipeline.rs`)

Current pipeline:
```
source → encoder → rtph264pay → tee → [webrtcbin branches]
```

New pipeline (when `--relay-url` is provided):
```
source → encoder → tee (raw H.264)
                    ├─ queue → rtph264pay → tee → [webrtcbin branches]  (local viewers, unchanged)
                    └─ queue → h264parse → flvmux streamable=true → rtmpsink  (RTMP push to MediaMTX)
```

The tee moves before `rtph264pay` so we can branch off raw H.264 for RTMP (which needs FLV encapsulation, not RTP).

**Note**: `whipclientsink` (WHIP) is unavailable because `main.rs:25` disables `rswebrtc` plugins due to glib type registration conflicts. RTMP avoids this entirely and is simpler.

### Step 2: Add Config Options (`src/config.rs`)

```rust
#[arg(long)]
pub relay_url: Vec<String>,  // e.g. rtmp://mediamtx.example.com/stream-0
```

If `relay_url` is empty, no RTMP push — behaves exactly as today.

### Step 3: Wire Relay Config Through (`src/main.rs`)

Pass relay URLs to `pipeline::setup_all_streams`. Map each stream index to its relay URL (by index or 1:1 if same count).

### Step 4: Deploy MediaMTX on AKS (via ArgoCD)

Manifests live in `k8s/mediamtx/` in this repo. ArgoCD syncs from Git — no manual `kubectl apply`.

```
k8s/
  mediamtx/
    configmap.yaml      # mediamtx.yml (HLS settings, path config, auth)
    secret.yaml         # RTMP publish credentials (SealedSecret or ExternalSecret in prod)
    deployment.yaml     # Single replica, 500m CPU / 512Mi RAM
    service-hls.yaml    # ClusterIP for HLS/WHEP (behind Ingress)
    service-rtmp.yaml   # LoadBalancer for RTMP ingress (port 1935)
    ingress.yaml        # TLS via cert-manager, routes /stream-* to HLS port
```

Key MediaMTX config:
- `hlsVariant: lowLatency` for LL-HLS (~1-3s latency)
- Wildcard path config so any published stream name is accepted
- RTMP publish authentication via username/password

ArgoCD Application resource (or add to existing app-of-apps) pointing to `k8s/mediamtx/` path in this repo.

### Step 5: DNS + TLS

- Point `streams.yourdomain.com` to the Ingress external IP
- cert-manager + Let's Encrypt for automatic TLS

### Step 6: TURN Server (Optional)

TURN is **only needed for WHEP** (WebRTC viewers) behind restrictive firewalls. **HLS needs no TURN** — it's plain HTTPS.

Since MediaMTX runs on AKS with a public IP, most WebRTC connections succeed with STUN alone. Add coturn only if WHEP connection failures are observed.

---

## Security

- RTMP LoadBalancer: restrict source IPs via `loadBalancerSourceRanges` to the local PC's public IP
- RTMP publish auth: username/password in MediaMTX config
- HLS/WHEP served over HTTPS via Ingress TLS

## Scaling

- 10-50 viewers: single MediaMTX instance is fine
- 50-100+: put Azure CDN or Cloudflare in front of HLS (segments are cache-friendly)

---

## Files to Modify

| File | Change |
|------|--------|
| `src/config.rs` | Add `--relay-url` CLI option |
| `src/pipeline.rs` | Restructure tee, add RTMP output branch |
| `src/main.rs` | Pass relay config to pipeline setup |
| `Cargo.toml` | No changes expected |

## New Files to Create

```
k8s/
  mediamtx/
    configmap.yaml
    secret.yaml
    deployment.yaml
    service-hls.yaml
    service-rtmp.yaml
    ingress.yaml
```

## Verification

1. Run MediaMTX locally: `docker run --rm -p 8888:8888 -p 1935:1935 bluenviron/mediamtx`
2. Run modified server: `cargo run -- --test-src --local --relay-url rtmp://localhost/stream-0`
3. Open `http://localhost:8888/stream-0/` in browser — should see HLS playback
4. Verify local WebRTC viewer still works at `http://localhost:8080`
5. Deploy to AKS and test with the public HLS URL
