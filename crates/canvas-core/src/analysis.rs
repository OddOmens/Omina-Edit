//! Measuring pictures: the histogram (shown above the Develop panel) and Image ▸ Auto Tone,
//! Auto Contrast and Auto Color, which set levels from it.

use rayon::prelude::*;

use crate::adjust::Adjustment;
use crate::bitmap::Bitmap;

/// How many pixels have each value, per channel, counting only pixels that aren't transparent.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub red: [u32; 256],
    pub green: [u32; 256],
    pub blue: [u32; 256],
    pub luma: [u32; 256],
    pub total: u32,
}

impl Histogram {
    /// Counts a bitmap's pixels (straight values, so half-transparent edges count true to color).
    /// Big pictures are sampled on a grid, which keeps it quick and changes little.
    pub fn of(bitmap: &Bitmap) -> Histogram {
        let pixels = bitmap.width as usize * bitmap.height as usize;
        let step = ((pixels / 2_000_000).max(1) as f32).sqrt().ceil() as usize;
        let width = bitmap.width as usize;
        let rows: Vec<usize> = (0..bitmap.height as usize).step_by(step).collect();
        // Each task counts a band of rows into its own tally on the heap (a tally is 4 KB, too
        // big to pass around on worker threads' stacks), and the tallies are added up after.
        let parts: Vec<Box<Histogram>> = rows
            .par_chunks(64)
            .map(|band| {
                let mut part = Box::new(Histogram::empty());
                for &y in band {
                    for x in (0..width).step_by(step) {
                        let index = (y * width + x) * 4;
                        let alpha = bitmap.data[index + 3];
                        if alpha == 0 {
                            continue;
                        }
                        let straight = |channel: u8| ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as usize;
                        let (r, g, b) = (straight(bitmap.data[index]), straight(bitmap.data[index + 1]), straight(bitmap.data[index + 2]));
                        part.red[r] += 1;
                        part.green[g] += 1;
                        part.blue[b] += 1;
                        part.luma[(r * 54 + g * 183 + b * 19) >> 8] += 1;
                        part.total += 1;
                    }
                }
                part
            })
            .collect();
        let mut total = Histogram::empty();
        for part in parts {
            for value in 0..256 {
                total.red[value] += part.red[value];
                total.green[value] += part.green[value];
                total.blue[value] += part.blue[value];
                total.luma[value] += part.luma[value];
            }
            total.total += part.total;
        }
        total
    }

    pub fn empty() -> Histogram {
        Histogram { red: [0; 256], green: [0; 256], blue: [0; 256], luma: [0; 256], total: 0 }
    }

    /// The value (0–1) below which `fraction` of the pixels' brightness falls.
    pub fn percentile_luma(&self, fraction: f32) -> f32 {
        percentile(&self.luma, self.total, fraction)
    }
}

fn percentile(counts: &[u32; 256], total: u32, fraction: f32) -> f32 {
    if total == 0 {
        return if fraction < 0.5 { 0.0 } else { 1.0 };
    }
    let wanted = (total as f32 * fraction.clamp(0.0, 1.0)) as u64;
    let mut seen = 0u64;
    for (value, &count) in counts.iter().enumerate() {
        seen += u64::from(count);
        if seen > wanted {
            return value as f32 / 255.0;
        }
    }
    1.0
}

/// Which automatic correction Image ▸ Auto makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoCorrection {
    /// Stretches each channel on its own (also removes a color cast).
    Tone,
    /// Stretches all channels together (keeps the colors' balance).
    Contrast,
    /// Stretches each channel and moves the midtones of each to neutral gray.
    Color,
}

/// Works out the Curves adjustment Auto Tone, Auto Contrast or Auto Color would apply to a
/// picture, clipping 0.1% of pixels at each end as Photoshop does.
pub fn auto_correction(bitmap: &Bitmap, kind: AutoCorrection) -> Adjustment {
    let histogram = Histogram::of(bitmap);
    let clip = 0.001;
    let range = |counts: &[u32; 256]| (percentile(counts, histogram.total, clip), percentile(counts, histogram.total, 1.0 - clip));
    let curve = |(low, high): (f32, f32), middle: Option<f32>| {
        let high = high.max(low + 0.02);
        let mut points = vec![[low, 0.0], [high, 1.0]];
        if let Some(middle) = middle {
            // Moving the channel's own middle to the overall middle neutralizes a cast.
            let at = ((middle - low) / (high - low)).clamp(0.1, 0.9);
            points.insert(1, [low + (high - low) * at, 0.5]);
        }
        if low > 0.0 {
            points.insert(0, [0.0, 0.0]);
        }
        if high < 1.0 {
            points.push([1.0, 1.0]);
        }
        points
    };
    let identity = crate::adjust::identity_curve();
    match kind {
        AutoCorrection::Contrast => {
            let low = range(&histogram.red).0.min(range(&histogram.green).0).min(range(&histogram.blue).0);
            let high = range(&histogram.red).1.max(range(&histogram.green).1).max(range(&histogram.blue).1);
            Adjustment::Curves { master: curve((low, high), None), red: identity.clone(), green: identity.clone(), blue: identity }
        }
        AutoCorrection::Tone => Adjustment::Curves {
            master: identity,
            red: curve(range(&histogram.red), None),
            green: curve(range(&histogram.green), None),
            blue: curve(range(&histogram.blue), None),
        },
        AutoCorrection::Color => Adjustment::Curves {
            master: identity,
            red: curve(range(&histogram.red), Some(percentile(&histogram.red, histogram.total, 0.5))),
            green: curve(range(&histogram.green), Some(percentile(&histogram.green, histogram.total, 0.5))),
            blue: curve(range(&histogram.blue), Some(percentile(&histogram.blue, histogram.total, 0.5))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    #[test]
    fn counts_what_is_there() {
        let mut bitmap = Bitmap::filled(10, 10, Rgba::rgb(1.0, 0.0, 0.0));
        bitmap.set_pixel(0, 0, [0, 0, 0, 0]);
        let histogram = Histogram::of(&bitmap);
        assert_eq!(histogram.total, 99, "transparent pixels don't count");
        assert_eq!(histogram.red[255], 99);
        assert_eq!(histogram.green[0], 99);
    }

    #[test]
    fn auto_contrast_stretches_a_dull_picture() {
        let mut bitmap = Bitmap::new(16, 16);
        for x in 0..16 {
            for y in 0..16 {
                let v = 100 + (x * 3) as u8;
                bitmap.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        let mut pixels = bitmap.data.clone();
        auto_correction(&bitmap, AutoCorrection::Contrast).apply(&mut pixels);
        let stretched = Bitmap { data: pixels, ..bitmap };
        let histogram = Histogram::of(&stretched);
        assert!(histogram.percentile_luma(0.01) < 0.05 && histogram.percentile_luma(0.99) > 0.95);
    }
}
