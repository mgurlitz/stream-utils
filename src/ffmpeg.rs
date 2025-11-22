use crate::commands::run_segment_command;
use chrono::Local;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use url::Url;

#[cfg(target_os = "linux")]
pub fn spawn_inotify_watcher(
    file_extension: String,
    output_dir: PathBuf,
    on_segment: Option<String>,
    verbose: bool,
    total_bytes_counter: Arc<AtomicU64>,
) {
    use tokio_stream::StreamExt;

    tokio::task::spawn(async move {
        let inotify = inotify::Inotify::init().expect("Failed to initialize inotify");
        inotify
            .watches()
            .add(&output_dir, inotify::WatchMask::CLOSE_WRITE)
            .expect("Failed to add watch");

        let mut buffer = [0u8; 4096];
        let mut stream = inotify
            .into_event_stream(&mut buffer)
            .expect("Failed to create event stream");

        while let Some(event_or_error) = stream.next().await {
            let event = match event_or_error {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("inotify error: {e}");
                    continue;
                }
            };

            if let Some(name) = event.name {
                let filename = name.to_string_lossy().to_string();
                // Only process .ext files
                if filename.ends_with(format!(".{}", file_extension).as_str()) {
                    let filepath = output_dir.join(&filename);

                    // Get file size before running command (which might delete it)
                    if let Ok(metadata) = std::fs::metadata(&filepath) {
                        total_bytes_counter.fetch_add(metadata.len(), Ordering::SeqCst);
                    }

                    if let Some(ref cmd) = on_segment {
                        run_segment_command(cmd, &filepath, verbose);
                    }
                }
            }
        }
    });
}

/// Handle fMP4 streams by shelling out to FFmpeg.
/// fMP4 requires proper demuxing that's complex to do manually.
pub fn run_ffmpeg_fmp4(
    media_url: &Url,
    file_extension: &str,
    output_dir: &PathBuf,
    segment_secs: u64,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let start_time = Local::now();
    let timestamp_prefix = start_time.format("%Y_%m_%d-%H_%M").to_string();

    // Find first available segment index (don't overwrite existing files)
    let mut start_index: u32 = 0;
    loop {
        let filename = format!("{}_{}.{}", timestamp_prefix, start_index, file_extension);
        let path = output_dir.join(&filename);
        if !path.exists() {
            break;
        }
        start_index += 1;
    }

    let output_pattern = output_dir.join(format!("{}_%d.{}", timestamp_prefix, file_extension));

    if verbose {
        eprintln!("Detected fMP4 stream, using FFmpeg for demuxing...");
        eprintln!("Output pattern: {}", output_pattern.display());
        if start_index > 0 {
            eprintln!("Starting at segment index: {}", start_index);
        }
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-v",
        "error",
        "-i",
        media_url.as_str(),
        "-c",
        "copy",
        "-c:a",
        "copy",
        "-f",
        "segment",
        "-segment_time",
        &segment_secs.to_string(),
        "-segment_start_number",
        &start_index.to_string(),
        "-max_muxing_queue_size",
        "512",
        // "-reset_timestamps",
        // "1",
    ])
    .arg(output_pattern.to_str().unwrap());

    if verbose {
        eprintln!("Running: ffmpeg {:?}", cmd.get_args().collect::<Vec<_>>());
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(format!("FFmpeg exited with: {status}").into());
    }

    Ok(())
}
