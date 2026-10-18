//! What a media file is, on one line (from `ffprobe`).

/// **What the file is, on ONE line**: `video h264 320x240 · audio aac 22050 Hz
/// mono · 6.0 s · QuickTime / MOV · 91 kb/s`. ffprobe's `key=value` output was
/// served as is — thirteen lines for a two-stream clip, on every media read,
/// for a summary documented as "~30 tokens". Fields ffprobe did not report
/// are simply absent; the size is left out, the line above it already says it.
pub(crate) fn probe_line(raw: &str) -> String {
    let mut streams: Vec<Vec<(&str, &str)>> = Vec::new();
    let mut format: Vec<(&str, &str)> = Vec::new();
    for line in raw.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        let v = v.trim();
        if v.is_empty() || v == "N/A" {
            continue;
        }
        match k {
            "duration" | "size" | "bit_rate" | "format_long_name" => format.push((k, v)),
            // `codec_name` opens each stream's block in ffprobe's order.
            "codec_name" => streams.push(vec![(k, v)]),
            _ => match streams.last_mut() {
                Some(st) => st.push((k, v)),
                None => streams.push(vec![(k, v)]),
            },
        }
    }
    fn get<'a>(kv: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
    let mut parts: Vec<String> = Vec::new();
    for st in &streams {
        let mut p = vec![get(st, "codec_type").unwrap_or("stream").to_string()];
        if let Some(c) = get(st, "codec_name") {
            p.push(c.to_string());
        }
        if let (Some(w), Some(h)) = (get(st, "width"), get(st, "height")) {
            p.push(format!("{w}x{h}"));
        }
        if let Some(r) = get(st, "sample_rate") {
            p.push(format!("{r} Hz"));
        }
        if let Some(c) = get(st, "channels") {
            p.push(match c {
                "1" => "mono".to_string(),
                "2" => "stereo".to_string(),
                n => format!("{n} ch"),
            });
        }
        parts.push(p.join(" "));
    }
    if let Some(d) = get(&format, "duration").and_then(|d| d.parse::<f64>().ok()) {
        parts.push(format!("{d:.1} s"));
    }
    if let Some(f) = get(&format, "format_long_name") {
        parts.push(f.to_string());
    }
    if let Some(b) = get(&format, "bit_rate").and_then(|b| b.parse::<f64>().ok()) {
        parts.push(format!("{:.0} kb/s", b / 1000.0));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod probe_line_tests {
    use super::probe_line;

    #[test]
    fn a_probe_is_one_line() {
        let raw = "codec_name=h264\ncodec_type=video\nwidth=320\nheight=240\ncodec_name=aac\ncodec_type=audio\nsample_rate=22050\nchannels=1\nformat_long_name=QuickTime / MOV\nduration=6.000000\nsize=67991\nbit_rate=90654\n";
        assert_eq!(
            probe_line(raw),
            "video h264 320x240 · audio aac 22050 Hz mono · 6.0 s · QuickTime / MOV · 91 kb/s"
        );
        let audio = "codec_name=mp3\ncodec_type=audio\nsample_rate=44100\nchannels=2\nduration=N/A\nformat_long_name=MP2/3 (MPEG audio layer 2/3)\n";
        assert_eq!(
            probe_line(audio),
            "audio mp3 44100 Hz stereo · MP2/3 (MPEG audio layer 2/3)"
        );
        // ffprobe writes `N/A` for what it cannot tell; it is not a value.
        let unknown = "codec_name=mjpeg\ncodec_type=video\nwidth=N/A\nheight=N/A\n";
        assert_eq!(probe_line(unknown), "video mjpeg");
    }
}
