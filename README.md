# stream-utils

Downloads live or VOD m3u8 streams into locally chunked video files. TS segments are fetched and written natively; fMP4 streams are handed off to ffmpeg. Master playlists are resolved automatically -- best variant (resolution, then framerate) is selected without any manual intervention.

---

## Compatibility

- Tested only on Linux. Uses inotify for file-watching in fMP4 mode. Windows should work otherwise.
- ffmpeg must be installed if you're pulling fMP4 or non-TS streams. Not needed for standard TS playlists.

---

## CLI

```
m3u8-dl <URL> [OPTIONS]
```

The only required argument is the m3u8 URL. Everything else is optional.

### Output and segmentation

| Flag | Default | What it does |
|---|---|---|
| `-o, --output` | `.` | Directory to write files into. Created if missing. |
| `--segment-secs` | `3600` | Rotate to a new output file after this many seconds of stream time. |
| `--file-extension` | `ts` | Extension on output files. Change to `mp4` if you're muxing via ffmpeg. |

Output files are named by start timestamp and segment index:

```
2026_02_02-14_30_0.ts
2026_02_02-14_30_1.ts
...
```

### Hooks

These are the main way to wire the downloader into a larger pipeline. Both run as shell commands.

| Flag | Placeholders | When it runs |
|---|---|---|
| `--on-segment <cmd>` | `{}` -- replaced with the completed file's path | Once per rotated segment, after the file is flushed and closed. Runs async so it does not block the download. |
| `--on-exit <cmd>` | `%d` directory, `%t` duration (H:M:S), `%s` size (human), `%b` bytes, `%m` megabytes | Once, on clean exit or Ctrl-C, after the final segment is written. |

### Network tuning

| Flag | Default | What it does |
|---|---|---|
| `--timeout` | `15` | Total timeout in seconds per fetch, across all retries. |
| `--retries` | `2` | Number of retry attempts within that timeout budget. |
| `--retry-delay-ms` | `500` | Wait between retries. |
| `--poll-interval` | `2` | Seconds between playlist re-fetches on a live stream. |
| `--max-failures` | `2` | Consecutive playlist fetch failures before giving up. Set to `0` to retry forever. |

### Stream format and mode

| Flag | What it does |
|---|---|
| `--ffmpeg` | Force ffmpeg for muxing. Needed for fMP4 streams or audio-only (e.g. MP3). Usually auto-detected. |
| `--direct` | Skip m3u8 parsing entirely. Passes the URL straight to ffmpeg. Requires `--ffmpeg`. |

### Diagnostics

| Flag | What it does |
|---|---|
| `--verbose` | Logs segment fetches, rotations, playlist re-fetches. |
| `--progress` | Prints a dot per segment fetched. Quiet but shows it's alive. |

---

## Example: uploading segments to cloud storage

A recording service captures a live stream and pushes each completed chunk to a cloud share as soon as it's written. No post-processing step, no cron job -- `--on-segment` handles it inline.

```bash
m3u8-dl "https://live.example.com/capture/playlist.m3u8" \
  -o /mnt/recordings \
  --segment-secs 600 \
  --on-segment "rclone copyto {} /mnt/cloud-share/captures/{}" \
  --on-exit "echo 'capture finished: %t, %s' >> /var/log/captures.log" \
  --verbose
```

What's happening:

- Stream is pulled and written to `/mnt/recordings` in 10-minute chunks.
- Each time a chunk is closed out, `rclone copyto` fires with the full path to that file, copying it to the cloud share.
- When the stream ends (or you kill it), the exit hook logs the total duration and size.
- `--verbose` keeps you informed about what's being fetched and when files rotate, without spamming progress output.

If the stream drops and comes back, `--max-failures 0` will keep polling indefinitely instead of bailing after two bad fetches:

```bash
m3u8-dl "https://live.example.com/capture/playlist.m3u8" \
  -o /mnt/recordings \
  --segment-secs 600 \
  --max-failures 0 \
  --on-segment "rclone copyto {} /mnt/cloud-share/captures/{}" \
  --verbose
```
