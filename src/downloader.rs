use crate::commands::run_segment_command_async;
use crate::http_client::{fetch_with_retry, HttpClient};
use crate::output::OutputFile;
use m3u8_rs::{MediaPlaylist, Playlist};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub struct DownloadConfig {
    pub media_url: Url,
    pub output_dir: PathBuf,
    pub file_extension: String,
    pub segment_secs: u64,
    pub poll_interval: u64,
    pub max_failures: u32,
    pub timeout: Duration,
    pub retries: u32,
    pub retry_delay_ms: u64,
    pub on_segment: Option<String>,
    pub verbose: bool,
    pub progress: bool,
}

pub struct TsDownloader {
    config: DownloadConfig,
    output: OutputFile,
    seen_segments: HashSet<String>,
    consecutive_failures: u32,
}

impl TsDownloader {
    pub fn new(config: DownloadConfig) -> std::io::Result<Self> {
        let output = OutputFile::new(
            config.file_extension.clone(),
            config.output_dir.clone(),
            Duration::from_secs(config.segment_secs),
            config.verbose,
        )?;

        Ok(Self {
            config,
            output,
            seen_segments: HashSet::new(),
            consecutive_failures: 0,
        })
    }

    pub async fn run(
        &mut self,
        client: &HttpClient,
        shutdown: Arc<AtomicBool>,
    ) -> Result<(u64, Vec<tokio::task::JoinHandle<()>>), Box<dyn std::error::Error + Send + Sync>>
    {
        let mut finalized = false;
        let mut pending_commands: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        loop {
            // Check for shutdown signal
            if shutdown.load(Ordering::SeqCst) {
                let final_path = self.output.finalize()?;
                finalized = true;
                eprintln!("Flushed current segment: {}", final_path.display());
                if let Some(ref cmd) = self.config.on_segment {
                    let handle =
                        run_segment_command_async(cmd.clone(), final_path, self.config.verbose);
                    pending_commands.push(handle);
                }
                break;
            }

            let media_data = match fetch_with_retry(
                client,
                self.config.media_url.as_str(),
                self.config.timeout,
                self.config.retries,
                self.config.retry_delay_ms,
            )
            .await
            {
                Ok(data) => data,
                Err(e) => {
                    self.consecutive_failures += 1;
                    if self.config.max_failures > 0
                        && self.consecutive_failures >= self.config.max_failures
                    {
                        eprintln!("\nPlaylist fetch error: {e}");
                        eprintln!(
                            "Giving up after {} consecutive failures",
                            self.consecutive_failures
                        );
                        break;
                    }
                    eprintln!(
                        "\nPlaylist fetch error (retrying {}/{}): {e}",
                        self.consecutive_failures, self.config.max_failures
                    );
                    tokio::time::sleep(Duration::from_secs(self.config.poll_interval)).await;
                    continue;
                }
            };

            let media_playlist: MediaPlaylist = match m3u8_rs::parse_playlist(&media_data) {
                Ok((_, Playlist::MediaPlaylist(pl))) => pl,
                _ => {
                    self.consecutive_failures += 1;
                    if self.config.max_failures > 0
                        && self.consecutive_failures >= self.config.max_failures
                    {
                        eprintln!("\nFailed to parse media playlist");
                        eprintln!(
                            "Giving up after {} consecutive failures",
                            self.consecutive_failures
                        );
                        break;
                    }
                    eprintln!(
                        "\nFailed to parse media playlist (retrying {}/{})",
                        self.consecutive_failures, self.config.max_failures
                    );
                    tokio::time::sleep(Duration::from_secs(self.config.poll_interval)).await;
                    continue;
                }
            };

            // Reset failure counter on successful fetch+parse
            self.consecutive_failures = 0;

            for segment in &media_playlist.segments {
                // Check for shutdown between segments
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                if self.seen_segments.contains(&segment.uri) {
                    continue;
                }
                self.seen_segments.insert(segment.uri.clone());

                let segment_url = self.config.media_url.join(&segment.uri)?;
                if self.config.progress {
                    eprint!(".");
                }

                match fetch_with_retry(
                    client,
                    segment_url.as_str(),
                    self.config.timeout,
                    self.config.retries,
                    self.config.retry_delay_ms,
                )
                .await
                {
                    Ok(data) => {
                        self.output.write(&data)?;
                        if let Some(completed_path) =
                            self.output.maybe_rotate(self.config.verbose)?
                        {
                            if let Some(ref cmd) = self.config.on_segment {
                                let handle = run_segment_command_async(
                                    cmd.clone(),
                                    completed_path,
                                    self.config.verbose,
                                );
                                pending_commands.push(handle);
                            }
                        }
                    }
                    Err(e) => eprintln!("\nSegment error (giving up): {e}"),
                }
            }

            // For live streams, keep polling; for VOD, exit when done
            if media_playlist.end_list {
                let final_path = self.output.finalize()?;
                finalized = true;
                if let Some(ref cmd) = self.config.on_segment {
                    let handle =
                        run_segment_command_async(cmd.clone(), final_path, self.config.verbose);
                    pending_commands.push(handle);
                }
                eprintln!("\nStream ended.");
                break;
            }

            tokio::time::sleep(Duration::from_secs(self.config.poll_interval)).await;
        }

        // Ensure we finalize and call on_segment for any exit path that didn't already
        if !finalized {
            let final_path = self.output.finalize()?;
            eprintln!("Flushed current segment: {}", final_path.display());
            if let Some(ref cmd) = self.config.on_segment {
                let handle =
                    run_segment_command_async(cmd.clone(), final_path, self.config.verbose);
                pending_commands.push(handle);
            }
        }

        // Wait for all pending on_segment commands to complete before exiting (with timeout)
        if !pending_commands.is_empty() {
            let unfinished = pending_commands.iter().filter(|p| !p.is_finished()).count();
            if unfinished > 0 {
                eprintln!("Waiting for {} pending commands to complete...", unfinished);
                for handle in &mut pending_commands {
                    if !handle.is_finished() {
                        match tokio::time::timeout(Duration::from_secs(60), handle).await {
                            Ok(_) => {}
                            Err(_) => eprintln!("Warning: on_segment command timed out after 60s"),
                        }
                    }
                }
            }
        }

        let total_bytes = self.output.total_bytes();
        Ok((total_bytes, pending_commands))
    }
}
