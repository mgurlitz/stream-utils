use chrono::Local;
use futures::StreamExt;
use mp4::{AacConfig, AvcConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig};
use retina::client::{SessionGroup, SetupOptions};
use retina::codec::{CodecItem, ParametersRef};
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub struct RtspConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub output_dir: PathBuf,
    pub segment_secs: u64,
    pub on_segment: Option<String>,
    pub verbose: bool,
    pub progress: bool,
}

/// Extract SPS and PPS from AVCC extra_data
fn parse_avcc(extra: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if extra.len() < 8 {
        return None;
    }
    let mut pos = 5;
    let num_sps = (extra[pos] & 0x1F) as usize;
    pos += 1;

    let mut sps = Vec::new();
    for _ in 0..num_sps {
        if pos + 2 > extra.len() { return None; }
        let len = u16::from_be_bytes([extra[pos], extra[pos + 1]]) as usize;
        pos += 2;
        if pos + len > extra.len() { return None; }
        sps = extra[pos..pos + len].to_vec();
        pos += len;
    }

    if pos >= extra.len() { return None; }
    let num_pps = extra[pos] as usize;
    pos += 1;

    let mut pps = Vec::new();
    for _ in 0..num_pps {
        if pos + 2 > extra.len() { return None; }
        let len = u16::from_be_bytes([extra[pos], extra[pos + 1]]) as usize;
        pos += 2;
        if pos + len > extra.len() { return None; }
        pps = extra[pos..pos + len].to_vec();
        pos += len;
    }

    if !sps.is_empty() && !pps.is_empty() {
        Some((sps, pps))
    } else {
        None
    }
}

struct Segment {
    writer: Mp4Writer<BufWriter<File>>,
    path: PathBuf,
    has_audio: bool,
}

pub async fn handle_rtsp_stream(
    config: RtspConfig,
    shutdown: Arc<AtomicBool>,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let creds = match (&config.username, &config.password) {
        (Some(u), Some(p)) => Some(retina::client::Credentials {
            username: u.clone(),
            password: p.clone(),
        }),
        _ => None,
    };

    let session_group = Arc::new(SessionGroup::default());
    let mut session = retina::client::Session::describe(
        url::Url::parse(&config.url)?,
        retina::client::SessionOptions::default()
            .creds(creds)
            .session_group(session_group)
            .user_agent("stream-utils/1.0".to_owned()),
    )
    .await?;

    if config.verbose {
        eprintln!("RTSP session established");
    }

    // Find video stream
    let video_idx = session
        .streams()
        .iter()
        .position(|s| s.media() == "video")
        .ok_or("No video stream found")?;

    session.setup(video_idx, SetupOptions::default()).await?;

    // Find and setup audio stream (optional)
    let audio_idx = session
        .streams()
        .iter()
        .position(|s| s.media() == "audio");

    if let Some(idx) = audio_idx {
        let _ = session.setup(idx, SetupOptions::default()).await;
    }

    // Get video params
    let (width, height, sps, pps) = session.streams()[video_idx]
        .parameters()
        .and_then(|p| {
            if let ParametersRef::Video(vp) = p {
                let (w, h) = vp.pixel_dimensions();
                let extra = vp.extra_data();
                if let Some((sps, pps)) = parse_avcc(extra) {
                    Some((w as u16, h as u16, sps, pps))
                } else {
                    Some((w as u16, h as u16, Vec::new(), Vec::new()))
                }
            } else {
                None
            }
        })
        .unwrap_or((1920, 1080, Vec::new(), Vec::new()));

    // Get audio params (if audio stream exists)
    let audio_params: Option<u32> = audio_idx.and_then(|idx| {
        session.streams()[idx].parameters().and_then(|p| {
            if let ParametersRef::Audio(ap) = p {
                Some(ap.clock_rate())
            } else {
                None
            }
        })
    });

    if config.verbose {
        eprintln!("Video: {}x{}, SPS: {} bytes, PPS: {} bytes", width, height, sps.len(), pps.len());
        if audio_params.is_some() {
            eprintln!("Audio: enabled");
        }
    }

    let mut session = session
        .play(retina::client::PlayOptions::default().initial_timestamp(retina::client::InitialTimestampPolicy::Permissive))
        .await?
        .demuxed()?;

    if config.verbose {
        eprintln!("Playback started");
    }

    let mut total_bytes: u64 = 0;
    let mut segment: Option<Segment> = None;
    let mut segment_start = Instant::now();
    let segment_duration = std::time::Duration::from_secs(config.segment_secs);
    let mut video_sample_time: u64 = 0;
    let mut audio_sample_time: u64 = 0;

    while let Some(item) = session.next().await {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        match item? {
            CodecItem::VideoFrame(frame) => {
                let is_key = frame.is_random_access_point();
                let data = frame.data();

                // Rotate segment on keyframe after duration
                let need_new = segment.is_none()
                    || (is_key && segment_start.elapsed() >= segment_duration);

                if need_new {
                    // Close old segment
                    if let Some(mut seg) = segment.take() {
                        seg.writer.write_end()?;
                        if let Some(ref cmd) = config.on_segment {
                            let cmd = cmd.replace("{}", &seg.path.to_string_lossy());
                            tokio::spawn(async move {
                                let _ = tokio::process::Command::new("sh")
                                    .arg("-c").arg(&cmd).status().await;
                            });
                        }
                    }

                    // New segment
                    let ts = Local::now().format("%Y%m%d_%H%M%S");
                    let path = config.output_dir.join(format!("{}.mp4", ts));
                    if config.verbose {
                        eprintln!("New segment: {}", path.display());
                    }

                    let file = BufWriter::new(File::create(&path)?);
                    let mp4_config = Mp4Config {
                        major_brand: str::parse("isom").unwrap(),
                        minor_version: 512,
                        compatible_brands: vec![
                            str::parse("isom").unwrap(),
                            str::parse("iso2").unwrap(),
                            str::parse("avc1").unwrap(),
                            str::parse("mp41").unwrap(),
                        ],
                        timescale: 90000,
                    };

                    let mut writer = Mp4Writer::write_start(file, &mp4_config)?;

                    let track_config = TrackConfig {
                        track_type: mp4::TrackType::Video,
                        timescale: 90000,
                        language: "und".to_string(),
                        media_conf: MediaConfig::AvcConfig(AvcConfig {
                            width,
                            height,
                            seq_param_set: sps.clone(),
                            pic_param_set: pps.clone(),
                        }),
                    };
                    writer.add_track(&track_config)?;

                    // Add audio track if available
                    let has_audio = if let Some(sample_rate) = audio_params {
                        let audio_config = TrackConfig {
                            track_type: mp4::TrackType::Audio,
                            timescale: sample_rate,
                            language: "und".to_string(),
                            media_conf: MediaConfig::AacConfig(AacConfig {
                                bitrate: 128000,
                                profile: mp4::AudioObjectType::AacLowComplexity,
                                freq_index: mp4::SampleFreqIndex::Freq48000,
                                chan_conf: mp4::ChannelConfig::Stereo,
                            }),
                        };
                        writer.add_track(&audio_config).is_ok()
                    } else {
                        false
                    };

                    segment = Some(Segment { writer, path, has_audio });
                    segment_start = Instant::now();
                    video_sample_time = 0;
                    audio_sample_time = 0;
                }

                if let Some(ref mut seg) = segment {
                    let sample = Mp4Sample {
                        start_time: video_sample_time,
                        duration: 3000, // ~30fps at 90kHz timescale
                        rendering_offset: 0,
                        is_sync: is_key,
                        bytes: mp4::Bytes::copy_from_slice(data),
                    };
                    seg.writer.write_sample(1, &sample)?;
                    total_bytes += data.len() as u64;
                    video_sample_time += 3000;

                    if config.progress {
                        eprint!(".");
                    }
                }
            }
            CodecItem::AudioFrame(frame) => {
                if let Some(ref mut seg) = segment {
                    if seg.has_audio {
                        let data = frame.data();
                        let sample = Mp4Sample {
                            start_time: audio_sample_time,
                            duration: 1024, // typical AAC frame duration
                            rendering_offset: 0,
                            is_sync: true,
                            bytes: mp4::Bytes::copy_from_slice(data),
                        };
                        let _ = seg.writer.write_sample(2, &sample); // track 2 = audio
                        total_bytes += data.len() as u64;
                        audio_sample_time += 1024;
                    }
                }
            }
            _ => {}
        }
    }

    // Close final segment
    if let Some(mut seg) = segment.take() {
        seg.writer.write_end()?;
        if let Some(ref cmd) = config.on_segment {
            let cmd = cmd.replace("{}", &seg.path.to_string_lossy());
            let _ = tokio::process::Command::new("sh")
                .arg("-c").arg(&cmd).status().await;
        }
    }

    Ok(total_bytes)
}
