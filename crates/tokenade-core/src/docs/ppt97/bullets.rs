//! **Bullet and level of each paragraph of a `.ppt`** ([MS-PPT] §2.9.44
//! `TextPFRun`, §2.9.18 `TextPFException`).
//!
//! The `StyleTextPropAtom` that follows a text splits its characters into runs of
//! paragraphs: each states its level (`indentLevel`) and, when its mask
//! declares it, whether it carries a bullet (`bulletFlags.fHasBullet`). Without reading it, the
//! body of a slide that LibreOffice exports as a plain text box
//! came out as flat lines, where the `.pptx` and the `.odp` of the same file
//! gave bullets.

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// What the style says about a paragraph: its bullet (`None`: inherited, unstated)
/// and its level.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Para {
    pub bullet: Option<bool>,
    pub level: usize,
}

/// The paragraph runs: (number of characters, what they say).
fn pf_runs(style: &[u8]) -> Vec<(usize, Para)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let (Some(count), Some(level), Some(masks)) = (
        u32_at(style, at),
        u16_at(style, at + 4),
        u32_at(style, at + 6),
    ) {
        at += 10;
        let bit = |b: u32| masks & (1 << b) != 0;
        let mut bullet = None;
        // The fields present, in the order of the specification.
        if masks & 0xF != 0 {
            let Some(flags) = u16_at(style, at) else {
                break;
            };
            if bit(0) {
                bullet = Some(flags & 1 != 0);
            }
            at += 2;
        }
        for (b, size) in [
            (7, 2),
            (4, 2),
            (6, 2),
            (5, 4),
            (11, 2),
            (12, 2),
            (13, 2),
            (14, 2),
            (8, 2),
            (10, 2),
            (9, 2),
        ] {
            if bit(b) {
                at += size;
            }
        }
        if bit(20) {
            let Some(stops) = u16_at(style, at) else {
                break;
            };
            at += 2 + stops * 4;
        }
        if bit(16) {
            at += 2;
        }
        if bit(17) || bit(18) || bit(19) {
            at += 2;
        }
        if bit(21) {
            at += 2;
        }
        out.push((
            count,
            Para {
                bullet,
                level: level.min(8),
            },
        ));
        // The CHARACTER runs follow: we stop at the text's total,
        // which the caller bounds by the number of paragraphs.
        if count == 0 {
            break;
        }
    }
    out
}

/// The style of each paragraph, by its start position (in units of the
/// text: UTF-16 or bytes).
pub(super) fn paragraph_styles(style: &[u8], starts: &[usize]) -> Vec<Para> {
    let runs = pf_runs(style);
    starts
        .iter()
        .map(|&start| {
            let mut end = 0;
            runs.iter()
                .find(|(count, _)| {
                    end += count;
                    start < end
                })
                .map(|(_, p)| *p)
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(count: u32, level: u16, masks: u32, fields: &[u8]) -> Vec<u8> {
        let mut v = count.to_le_bytes().to_vec();
        v.extend(level.to_le_bytes());
        v.extend(masks.to_le_bytes());
        v.extend(fields);
        v
    }

    /// A bulleted paragraph, then a level-1 one whose run also carries an
    /// alignment and tab stops to skip, then one that says no bullet.
    #[test]
    fn each_paragraph_reads_its_bullet_and_level() {
        let mut style = run(6, 0, 0x1, &[1, 0]);
        style.extend(run(
            4,
            1,
            0x1 | 1 << 11 | 1 << 20,
            &[1, 0, 2, 0, 1, 0, 0, 0, 0, 0],
        ));
        style.extend(run(3, 0, 0x1, &[0, 0]));
        style.extend(run(2, 0, 0, &[]));
        let paras = paragraph_styles(&style, &[0, 6, 10, 13]);
        assert_eq!(
            paras,
            [
                Para {
                    bullet: Some(true),
                    level: 0
                },
                Para {
                    bullet: Some(true),
                    level: 1
                },
                Para {
                    bullet: Some(false),
                    level: 0
                },
                Para {
                    bullet: None,
                    level: 0
                },
            ]
        );
    }
}
