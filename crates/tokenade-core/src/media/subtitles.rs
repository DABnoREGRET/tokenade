//! Subtitles (srt/vtt, sidecar, embedded or ASR) rendered as a transcript.

/// Strip subtitle markup to plain, readable text: drop cue indices and the
/// WEBVTT header, keep timestamps (an agent asking "when did they say X" needs
/// them), collapse blank runs. Works for srt and vtt.
pub fn subtitle_to_text(raw: &str) -> String {
    let all: Vec<&str> = raw.lines().collect();
    // Everything before the first cue is the file's header — `WEBVTT`,
    // `Kind: captions`, `Language: en`, a `STYLE` block — not speech.
    let Some(first) = all.iter().position(|l| l.contains(" --> ")) else {
        return String::new();
    };
    let start = if first > 0
        && all[first - 1].trim().chars().all(|c| c.is_ascii_digit())
        && !all[first - 1].trim().is_empty()
    {
        first - 1
    } else {
        first
    };
    let tag = tk_base::regexes::re(r"<[^>]*>");
    let mut out = String::new();
    // The last lines served: auto-generated captions ROLL — each cue repeats
    // the line before the new one, and an interstitial 10 ms cue repeats it
    // again — so a sentence came out two or three times, still wrapped in
    // its word-timing tags (`welcome<00:00:00.500><c> to</c>`).
    let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut stamp: Option<String> = None;
    let mut cue: Vec<String> = Vec::new();
    // Cues are gathered into PARAGRAPHS of up to `PARAGRAPH_SECS`, one
    // timestamp each: a cue is ~3 s and ten words, and its own `HH:MM:SS`
    // plus blank line cost a quarter of a long transcript (26 stamps for 342
    // words on a 2-minute recording, 2026-10-04). Half a minute still says
    // where a thing is said.
    let mut para_start: Option<u64> = None;
    let mut flush = |stamp: &mut Option<String>, cue: &mut Vec<String>, out: &mut String| {
        if !cue.is_empty() {
            let at = stamp.as_deref().and_then(stamp_secs);
            let new_para = match (para_start, at) {
                (Some(p), Some(a)) => a >= p + PARAGRAPH_SECS,
                (None, _) => true,
                (Some(_), None) => false,
            };
            if new_para {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                if let Some(st) = stamp.as_deref() {
                    out.push_str(st);
                    out.push(' ');
                }
                para_start = at.or(para_start);
            } else {
                out.push(' ');
            }
            out.push_str(&cue.join(" "));
        }
        cue.clear();
        *stamp = None;
    };
    for (idx, line) in all.iter().enumerate().skip(start) {
        let t = line.trim();
        let next_is_timestamp = all.get(idx + 1).is_some_and(|n| n.contains(" --> "));
        if t.is_empty() {
            continue;
        }
        if t.starts_with("NOTE") && !t.contains(" --> ") {
            continue;
        }
        // A bare integer is an srt cue index — noise. But ONLY when the next
        // line is its timestamp: a cue whose spoken text is just a number
        // ("2024") was being deleted from the transcript.
        if t.chars().all(|c| c.is_ascii_digit()) && next_is_timestamp {
            continue;
        }
        // Keep the start timestamp only; the end adds cost without answering
        // anything an agent asks.
        if let Some((st, _)) = t.split_once(" --> ") {
            flush(&mut stamp, &mut cue, &mut out);
            // SRT uses a COMMA for milliseconds, VTT a dot — splitting on '.'
            // alone left "00:00:04,120" untrimmed on every SRT.
            let cut = st.find(['.', ',']).unwrap_or(st.len());
            stamp = Some(st[..cut].trim().to_string());
            continue;
        }
        let text = tag.replace_all(t, "");
        let text = text
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&nbsp;", " ");
        let text = text.trim();
        if text.is_empty() || recent.iter().any(|r| r == text) {
            continue;
        }
        recent.push_back(text.to_string());
        if recent.len() > 2 {
            recent.pop_front();
        }
        cue.push(text.to_string());
    }
    flush(&mut stamp, &mut cue, &mut out);
    out.trim_end().to_string() + "\n"
}

/// Longest span of speech gathered under one timestamp.
const PARAGRAPH_SECS: u64 = 30;

/// `HH:MM:SS` (or `MM:SS`) as seconds.
fn stamp_secs(st: &str) -> Option<u64> {
    st.split(':').try_fold(0u64, |acc, p| {
        p.trim().parse::<u64>().ok().map(|n| acc * 60 + n)
    })
}

#[cfg(test)]
mod rolling_caption_tests {
    use super::subtitle_to_text;

    /// YouTube's auto-captions: a header, word-timing tags, and each line
    /// rolled into the next cue plus a 10 ms interstitial repeating it.
    #[test]
    fn rolling_auto_captions_read_once_without_tags() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
00:00:00.000 --> 00:00:02.000 align:start position:0%\nwelcome<00:00:00.500><c> to</c><00:00:01.000><c> the review</c>\n\n\
00:00:02.000 --> 00:00:02.010 align:start position:0%\nwelcome to the review\n \n\n\
00:00:02.010 --> 00:00:04.000 align:start position:0%\nwelcome to the review\nrevenue<00:00:02.500><c> grew</c>\n\n\
00:00:04.000 --> 00:00:04.010 align:start position:0%\nrevenue grew\n";
        assert_eq!(
            subtitle_to_text(vtt),
            "00:00:00 welcome to the review revenue grew\n"
        );
    }

    /// Cues gather under one timestamp until 30 s have passed.
    #[test]
    fn cues_gather_into_half_minute_paragraphs() {
        // At exactly 30 s a new paragraph opens; across a minute boundary
        // (00:00:50 → 00:01:05, fifteen seconds) it does not.
        let srt = "1\n00:00:00,000 --> 00:00:03,000\nFirst.\n\n2\n00:00:12,000 --> 00:00:15,000\nSecond.\n\n3\n00:00:30,000 --> 00:00:34,000\nThird.\n\n4\n00:01:05,000 --> 00:01:08,000\nFourth.\n\n5\n01:00:05,000 --> 01:00:08,000\nLater.\n";
        assert_eq!(
            subtitle_to_text(srt),
            "00:00:00 First. Second.\n\n00:00:30 Third.\n\n00:01:05 Fourth.\n\n01:00:05 Later.\n"
        );
        let near = "1\n00:00:50,000 --> 00:00:53,000\nA.\n\n2\n00:01:05,000 --> 00:01:08,000\nB.\n";
        assert_eq!(subtitle_to_text(near), "00:00:50 A. B.\n");
    }

    /// SRT keeps its timestamps, drops cue numbers — but not a cue whose
    /// spoken text is a number.
    #[test]
    fn srt_cues_and_a_spoken_number() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nTom &amp; Jerry\n\n2\n00:00:03,500 --> 00:00:04,000\n2024\n";
        assert_eq!(subtitle_to_text(srt), "00:00:01 Tom & Jerry 2024\n");
    }
}
