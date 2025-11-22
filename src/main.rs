mod cli;
mod commands;
mod downloader;
mod ffmpeg;
mod http_client;
mod output;
mod playlist;
#[cfg(feature = "rtsp")]
mod rtsp;

use clap::Parser;
use m3u8_rs::Playlist;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

use cli::Args;
use downloader::{DownloadConfig, TsDownloader};
use http_client::{build_client, fetch_with_retry, HttpClient};

fn setup_shutdown_handler() -> Arc<AtomicBool> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        eprintln!("\nReceived Ctrl+C, shutting down gracefully...");
        shutdown_clone.store(true, Ordering::SeqCst);
    });
    shutdown
}

async fn resolve_media_url(
    client: &HttpClient,
    args: &Args,
    timeout: Duration,
) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
    let base_url = Url::parse(&args.url)?;
    let data = fetch_with_retry(
        client,
        &args.url,
        timeout,
        args.retries,
        args.retry_delay_ms,
    )
    .await?;
    let playlist = m3u8_rs::parse_playlist(&data)
        .map_err(|e| format!("Parse error: {e:?}"))?
        .1;

    // Resolve to media playlist URL
    let media_url = match playlist {
        Playlist::MasterPlaylist(master) => {
            playlist::select_best_variant(&master, &base_url, args.verbose)
                .ok_or("No suitable variant found")?
        }
        Playlist::MediaPlaylist(_) => base_url,
    };

    Ok(media_url)
}

async fn detect_format(
    client: &HttpClient,
    media_url: &Url,
    timeout: Duration,
    retries: u32,
    retry_delay_ms: u64,
) -> Result<StreamFormat, Box<dyn std::error::Error + Send + Sync>> {
    // Fetch media playlist once to detect format
    let initial_media_data =
        fetch_with_retry(client, media_url.as_str(), timeout, retries, retry_delay_ms).await?;

    let initial_playlist: m3u8_rs::MediaPlaylist =
        match m3u8_rs::parse_playlist(&initial_media_data) {
            Ok((_, Playlist::MediaPlaylist(pl))) => pl,
            _ => return Err("Failed to parse media playlist".into()),
        };

    // Check if this is an fMP4 stream
    if playlist::is_fmp4_playlist(&initial_playlist) {
        Ok(StreamFormat::FMP4)
    } else {
        Ok(StreamFormat::TS)
    }
}

async fn handle_fmp4_stream(
    media_url: &Url,
    args: &Args,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let ffmpeg_bytes_counter = Arc::new(AtomicU64::new(0));

    #[cfg(target_os = "linux")]
    if args.on_segment.is_some() {
        ffmpeg::spawn_inotify_watcher(
            args.file_extension.clone(),
            args.output.clone(),
            args.on_segment.clone(),
            args.verbose,
            ffmpeg_bytes_counter.clone(),
        );
    }

    ffmpeg::run_ffmpeg_fmp4(
        media_url,
        &args.file_extension,
        &args.output,
        args.segment_secs,
        args.verbose,
    )?;

    Ok(ffmpeg_bytes_counter.load(Ordering::SeqCst))
}

async fn handle_ts_stream(
    client: &HttpClient,
    media_url: &Url,
    args: &Args,
    shutdown: Arc<AtomicBool>,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    if args.verbose {
        eprintln!("Detected TS stream, processing natively...");
    }

    let config = DownloadConfig {
        media_url: media_url.clone(),
        output_dir: args.output.clone(),
        file_extension: args.file_extension.clone(),
        segment_secs: args.segment_secs,
        poll_interval: args.poll_interval,
        max_failures: args.max_failures,
        timeout: Duration::from_secs(args.timeout),
        retries: args.retries,
        retry_delay_ms: args.retry_delay_ms,
        on_segment: args.on_segment.clone(),
        verbose: args.verbose,
        progress: args.progress,
    };

    let mut downloader = TsDownloader::new(config)?;
    let (total_bytes, _pending_commands) = downloader.run(client, shutdown).await?;

    Ok(total_bytes)
}

enum StreamFormat {
    FMP4,
    TS,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let recording_start = Instant::now();

    // Setup
    let client = build_client(args.insecure);
    let shutdown = setup_shutdown_handler();
    std::fs::create_dir_all(&args.output)?;

    // Check if this is an RTSP URL
    if args.url.starts_with("rtsp://") || args.url.starts_with("rtsps://") {
        #[cfg(feature = "rtsp")]
        {
            if args.verbose {
                eprintln!("Detected RTSP stream...");
            }
            let rtsp_config = rtsp::RtspConfig {
                url: args.url.clone(),
                username: args.username.clone(),
                password: args.password.clone(),
                output_dir: args.output.clone(),
                segment_secs: args.segment_secs,
                on_segment: args.on_segment.clone(),
                verbose: args.verbose,
                progress: args.progress,
            };

            let total_bytes = rtsp::handle_rtsp_stream(rtsp_config, shutdown).await?;

            if let Some(ref cmd) = args.on_exit {
                commands::run_exit_command(
                    cmd,
                    recording_start.elapsed().as_secs(),
                    total_bytes,
                    &args.output,
                    args.verbose,
                );
            }

            if args.fake_exit_err {
                std::process::exit(130);
            }

            return Ok(());
        }

        #[cfg(not(feature = "rtsp"))]
        {
            return Err("RTSP support not compiled in. Rebuild with --features rtsp".into());
        }
    }

    let timeout = Duration::from_secs(args.timeout);

    // Fetch and resolve playlist (skip if --direct)
    let media_url = if args.direct {
        Url::parse(&args.url)?
    } else {
        resolve_media_url(&client, &args, timeout).await?
    };

    // Detect format and dispatch (skip detection if --ffmpeg is set)
    let total_bytes = if args.ffmpeg || args.direct {
        if args.verbose {
            eprintln!("Forcing ffmpeg mode...");
        }
        handle_fmp4_stream(&media_url, &args).await?
    } else {
        let format = detect_format(
            &client,
            &media_url,
            timeout,
            args.retries,
            args.retry_delay_ms,
        )
        .await?;

        match format {
            StreamFormat::FMP4 => handle_fmp4_stream(&media_url, &args).await?,
            StreamFormat::TS => handle_ts_stream(&client, &media_url, &args, shutdown).await?,
        }
    };

    // Run on-exit command
    if let Some(ref cmd) = args.on_exit {
        commands::run_exit_command(
            cmd,
            recording_start.elapsed().as_secs(),
            total_bytes,
            &args.output,
            args.verbose,
        );
    }

    if args.fake_exit_err {
        std::process::exit(130);
    }

    Ok(())
}
