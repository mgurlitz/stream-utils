use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(
    name = "m3u8-dl",
    about = "Download m3u8 streams to chunked video files"
)]
pub struct Args {
    /// M3U8 URL to download
    pub url: String,

    /// Output directory
    #[arg(short, long, default_value = ".")]
    pub output: PathBuf,

    /// Segment duration in seconds (rotate file after this duration)
    #[arg(short, long, default_value = "3600")]
    pub segment_secs: u64,

    /// Fake an error on exit
    #[clap(long, action)]
    pub fake_exit_err: bool,

    /// Show progress dots
    #[clap(long, action)]
    pub progress: bool,

    /// Show verbose logs
    #[clap(long, action)]
    pub verbose: bool,

    /// Total timeout in seconds for a fetch operation (across all retries)
    #[arg(long, default_value = "15")]
    pub timeout: u64,

    /// Number of retries for failed requests (within the total timeout)
    #[arg(long, default_value = "2")]
    pub retries: u32,

    /// Delay in milliseconds between retry attempts
    #[arg(long, default_value = "500")]
    pub retry_delay_ms: u64,

    /// Playlist poll interval in seconds (for live streams)
    #[arg(long, default_value = "2")]
    pub poll_interval: u64,

    /// Maximum consecutive playlist fetch/parse failures before giving up (0 = infinite)
    #[arg(long, default_value = "2")]
    pub max_failures: u32,

    /// Command to run after each segment file is completed.
    /// Use {} as placeholder for the filename (will be replaced).
    /// Example: --on-segment "ffmpeg -i {} -c copy /archive/{}"
    #[arg(long)]
    pub on_segment: Option<String>,

    /// Command to run when the program exits.
    /// Placeholders: %d = output directory (last 2 components), %t = total duration (H:M:S or M:S), %s = total size
    /// Example: --on-exit "notify-send 'Recording complete' 'Directory: %d, Duration: %t, Size: %s'"
    #[arg(long)]
    pub on_exit: Option<String>,

    /// File extension, ts by default
    #[arg(long, default_value = "ts")]
    pub file_extension: String,

    /// Force ffmpeg mode (useful for audio streams like MP3)
    #[clap(long, action)]
    pub ffmpeg: bool,

    /// Skip m3u8 parsing, pass URL directly to ffmpeg (use with --ffmpeg)
    #[clap(long, action)]
    pub direct: bool,

    /// Disable HTTPS certificate verification (insecure, use with caution)
    #[clap(long, action)]
    pub insecure: bool,

    /// Username for RTSP authentication
    #[arg(long)]
    pub username: Option<String>,

    /// Password for RTSP authentication
    #[arg(long)]
    pub password: Option<String>,
}
