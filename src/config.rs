use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "Cuda IPC to Webrtc")]
#[command(about = "WebRTC server that streams CUDA IPC texture output to browsers")]
pub struct Config {
    #[arg(long)]
    pub ipc_address: Vec<String>,

    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    #[arg(long, default_value_t = 4000)]
    pub bitrate: u32,

    #[arg(long, default_value_t = false)]
    pub test_src: bool,

    #[arg(long, default_value = "stun://stun.l.google.com:19302")]
    pub stun_server: String,

    #[arg(long, default_value_t = false)]
    pub local: bool,
}
