use m3u8_rs::{MasterPlaylist, MediaPlaylist};
use url::Url;

/// Try to extract FPS value from a string like "FPS:30.0" or containing "FPS:30.0"
fn parse_fps_from_string(s: &str) -> Option<f64> {
    // Try exact match first (e.g., "FPS:30.0")
    if let Some(fps_str) = s.strip_prefix("FPS:") {
        if let Ok(fps) = fps_str.parse::<f64>() {
            return Some(fps);
        }
    }
    // Try to find FPS: pattern anywhere in the string
    if let Some(idx) = s.find("FPS:") {
        let after = &s[idx + 4..];
        let fps_part: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(fps) = fps_part.parse::<f64>() {
            return Some(fps);
        }
    }
    None
}

/// Extract frame rate from variant - checks standard frame_rate field first,
/// then falls back to other_attributes NAME (e.g., NAME="FPS:30.0")
fn extract_frame_rate(variant: &m3u8_rs::VariantStream) -> f64 {
    // Standard FRAME-RATE attribute
    if let Some(fps) = variant.frame_rate {
        if fps > 0.0 {
            return fps;
        }
    }

    // Try other_attributes for non-standard NAME="FPS:X.X"
    if let Some(ref attrs) = variant.other_attributes {
        if let Some(name_val) = attrs.get("NAME") {
            let name_str = match name_val {
                m3u8_rs::QuotedOrUnquoted::Quoted(s) => s.as_str(),
                m3u8_rs::QuotedOrUnquoted::Unquoted(s) => s.as_str(),
            };
            if let Some(fps) = parse_fps_from_string(name_str) {
                return fps;
            }
        }
    }

    0.0
}

/// Check if a media playlist uses fMP4 (fragmented MP4) segments.
/// fMP4 streams have an EXT-X-MAP tag specifying an initialization segment.
pub fn is_fmp4_playlist(playlist: &MediaPlaylist) -> bool {
    // Check if any segment has a map (EXT-X-MAP) - this indicates fMP4
    playlist.segments.iter().any(|s| s.map.is_some())
}

/// Select best variant: highest resolution, then highest framerate at that resolution
pub fn select_best_variant(master: &MasterPlaylist, base_url: &Url, verbose: bool) -> Option<Url> {
    let best = master.variants.iter().max_by(|a, b| {
        let res_a = a.resolution.map(|r| r.width * r.height).unwrap_or(0);
        let res_b = b.resolution.map(|r| r.width * r.height).unwrap_or(0);
        let fps_a = extract_frame_rate(a);
        let fps_b = extract_frame_rate(b);

        res_a.cmp(&res_b).then_with(|| {
            fps_a
                .partial_cmp(&fps_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    })?;

    let variant_url = base_url.join(&best.uri).ok()?;
    if verbose {
        if let Some(res) = best.resolution {
            eprintln!(
                "Selected: {}x{} @ {:.1} fps",
                res.width,
                res.height,
                extract_frame_rate(best)
            );
        }
    }
    Some(variant_url)
}
