pub fn viewer_html(stun_server: &str) -> String {
    let ice_url = stun_server.replacen("stun://", "stun:", 1);

    VIEWER_HTML_TEMPLATE.replace("{{ICE_SERVER}}", &ice_url)
}

const VIEWER_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Oden WebRTC Viewer</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { background: #1a1a1a; color: #eee; font-family: system-ui, sans-serif; display: flex; flex-direction: column; align-items: center; min-height: 100vh; padding: 16px; }
  h1 { margin-bottom: 12px; font-size: 1.2em; color: #aaa; }
  #grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(480px, 1fr)); gap: 12px; width: 100%; max-width: 1920px; }
  .stream { background: #222; border-radius: 6px; overflow: hidden; }
  .stream-header { display: flex; justify-content: space-between; padding: 8px 12px; font-size: 0.85em; }
  .stream-label { color: #ccc; }
  .stream-status { color: #888; }
  .stream video { width: 100%; background: #000; display: block; }
</style>
</head>
<body>
<h1>Oden WebRTC Viewer</h1>
<div id="grid"></div>
<script>
async function init() {
  const res = await fetch('/streams');
  const streams = await res.json();
  const grid = document.getElementById('grid');

  streams.forEach(s => {
    const div = document.createElement('div');
    div.className = 'stream';
    div.innerHTML = `
      <div class="stream-header">
        <span class="stream-label">Stream ${s.id} &mdash; ${s.address}</span>
        <span class="stream-status" id="status-${s.id}">Connecting...</span>
      </div>
      <video id="video-${s.id}" autoplay playsinline muted></video>
    `;
    grid.appendChild(div);
    connectStream(s.id);
  });
}

function connectStream(id) {
  const status = document.getElementById(`status-${id}`);
  const video = document.getElementById(`video-${id}`);
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${proto}//${location.host}/ws/${id}`);
  let pc = null;

  ws.onopen = () => {
    status.textContent = 'Waiting for offer...';
  };

  ws.onclose = () => {
    status.textContent = 'Disconnected. Reconnecting...';
    if (pc) { pc.close(); pc = null; }
    setTimeout(() => connectStream(id), 2000);
  };

  ws.onerror = () => ws.close();

  ws.onmessage = async (ev) => {
    const msg = JSON.parse(ev.data);

    if (msg.type === 'offer') {
      status.textContent = 'Creating answer...';

      pc = new RTCPeerConnection({
        iceServers: [{ urls: '{{ICE_SERVER}}' }]
      });

      pc.ontrack = (e) => {
        status.textContent = 'Streaming';
        video.srcObject = e.streams[0];
      };

      pc.onicecandidate = (e) => {
        if (e.candidate) {
          ws.send(JSON.stringify({
            type: 'ice',
            candidate: e.candidate.candidate,
            sdpMLineIndex: e.candidate.sdpMLineIndex
          }));
        }
      };

      pc.oniceconnectionstatechange = () => {
        if (pc.iceConnectionState === 'connected') {
          status.textContent = 'Connected';
        } else if (pc.iceConnectionState === 'failed' || pc.iceConnectionState === 'disconnected') {
          status.textContent = `ICE: ${pc.iceConnectionState}`;
        }
      };

      await pc.setRemoteDescription({ type: 'offer', sdp: msg.sdp });
      const answer = await pc.createAnswer();
      await pc.setLocalDescription(answer);
      ws.send(JSON.stringify({ type: 'answer', sdp: answer.sdp }));

    } else if (msg.type === 'ice') {
      if (pc) {
        await pc.addIceCandidate({
          candidate: msg.candidate,
          sdpMLineIndex: msg.sdpMLineIndex
        });
      }
    }
  };
}

init();
</script>
</body>
</html>"#;
