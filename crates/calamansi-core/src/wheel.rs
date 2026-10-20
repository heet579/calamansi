//! What the wheel offers and how it is laid out.
//!
//! This is platform-neutral so the menu logic and hit-testing are unit
//! tested on every OS. The Windows shell only draws text on top of the
//! shapes rendered here and forwards mouse positions.

use crate::batch::{Action, Tool};
use crate::format::Format;
use crate::routes;

pub const MAX_SEGMENTS: usize = 12;

/// Smallest alpha a layered window still hit-tests (1/255).
pub const HIT_ALPHA: f32 = 1.0 / 255.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Shift-drag: convert to another format.
    Convert,
    /// Ctrl+Shift-drag: tools and file actions.
    Tools,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub action: Action,
}

/// Wheel entries for a selection.
pub fn items(mode: Mode, inputs: &[Format], ffmpeg_available: bool) -> Vec<Item> {
    let mut v: Vec<Item> = match mode {
        Mode::Convert => routes::targets_for(inputs, ffmpeg_available)
            .into_iter()
            .map(|(f, _)| Item {
                label: f.label().to_string(),
                action: Action::Convert(f),
            })
            .collect(),
        Mode::Tools => Tool::ALL
            .iter()
            .filter(|t| t.applies_to(inputs))
            .map(|&t| Item {
                label: t.label().to_string(),
                action: Action::Tool(t),
            })
            .collect(),
    };
    v.truncate(MAX_SEGMENTS);
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Center,
    Segment(usize),
    Outside,
}

/// Wheel dimensions in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub size: u32,
    pub r_outer: f32,
    pub r_inner: f32,
    pub gap: f32,
}

impl Geometry {
    pub fn for_scale(scale: f32) -> Geometry {
        let s = scale.clamp(0.5, 4.0);
        let size = (320.0 * s).round() as u32;
        Geometry {
            size,
            r_outer: 150.0 * s,
            r_inner: 62.0 * s,
            gap: 3.0 * s,
        }
    }

    fn center(&self) -> f32 {
        self.size as f32 / 2.0
    }

    /// Angle (radians, clockwise from 12 o'clock) of segment `i`'s middle.
    pub fn segment_angle(i: usize, n: usize) -> f32 {
        std::f32::consts::TAU * i as f32 / n.max(1) as f32
    }

    /// Where to put segment `i`'s label, relative to the wheel's top-left.
    pub fn label_point(&self, i: usize, n: usize) -> (f32, f32) {
        let a = Self::segment_angle(i, n);
        let r = (self.r_outer + self.r_inner) / 2.0;
        (self.center() + r * a.sin(), self.center() - r * a.cos())
    }

    /// What lies under (`x`, `y`), relative to the wheel's top-left.
    pub fn hit(&self, x: f32, y: f32, n: usize) -> Hit {
        let (dx, dy) = (x - self.center(), y - self.center());
        let r = (dx * dx + dy * dy).sqrt();
        if r < self.r_inner {
            Hit::Center
        } else if r > self.r_outer || n == 0 {
            Hit::Outside
        } else {
            Hit::Segment(segment_index(dx, dy, n))
        }
    }

    /// Render the wheel's shapes: straight (not premultiplied) RGB plus a
    /// separate alpha channel, row-major, `size × size`.
    pub fn render(&self, n: usize, hovered: Option<usize>, theme: &Theme) -> Canvas {
        let size = self.size as usize;
        let mut c = Canvas::new(size, size);
        let ctr = self.center();
        let seg = std::f32::consts::TAU / n.max(1) as f32;
        for y in 0..size {
            for x in 0..size {
                let (dx, dy) = (x as f32 + 0.5 - ctr, y as f32 + 0.5 - ctr);
                let r = (dx * dx + dy * dy).sqrt();
                // Centre disc.
                let disc = cover(self.r_inner - 4.0 * self.gap - r);
                if disc > 0.0 {
                    c.set(x, y, theme.center, disc * theme.alpha);
                    continue;
                }
                // Ring.
                let ring = cover(r - self.r_inner) * cover(self.r_outer - r);
                if ring <= 0.0 {
                    if r < self.r_outer {
                        // The gap around the centre disc: invisible but still
                        // part of the window, so a drag doesn't "leave" here.
                        c.set(x, y, theme.ring, HIT_ALPHA);
                    }
                    continue;
                }
                if n == 0 {
                    c.set(x, y, theme.ring, ring * theme.alpha);
                    continue;
                }
                let i = segment_index(dx, dy, n);
                // Distance in pixels from the nearest segment boundary.
                let a = clockwise_angle(dx, dy) + seg / 2.0;
                let off = a.rem_euclid(seg);
                let edge = off.min(seg - off) * r;
                let gap = if n > 1 {
                    cover(edge - self.gap / 2.0)
                } else {
                    1.0
                };
                let color = if Some(i) == hovered {
                    theme.highlight
                } else {
                    theme.ring
                };
                c.set(x, y, color, (ring * gap * theme.alpha).max(HIT_ALPHA));
            }
        }
        c
    }
}

/// Clockwise angle from 12 o'clock in `[0, τ)`.
fn clockwise_angle(dx: f32, dy: f32) -> f32 {
    dx.atan2(-dy).rem_euclid(std::f32::consts::TAU)
}

fn segment_index(dx: f32, dy: f32, n: usize) -> usize {
    let seg = std::f32::consts::TAU / n as f32;
    (((clockwise_angle(dx, dy) + seg / 2.0) / seg).floor() as usize) % n
}

/// Anti-aliasing coverage for a signed distance (positive = inside).
fn cover(d: f32) -> f32 {
    (d + 0.5).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub ring: [u8; 3],
    pub highlight: [u8; 3],
    pub center: [u8; 3],
    pub text: [u8; 3],
    pub alpha: f32,
}

impl Theme {
    pub const DARK: Theme = Theme {
        ring: [30, 30, 32],
        highlight: [101, 163, 13],
        center: [48, 48, 52],
        text: [245, 245, 245],
        alpha: 0.94,
    };
}

/// Straight RGB with separate coverage, so text can be drawn over opaque
/// colour first and alpha applied afterwards.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[u8; 3]>,
    pub alpha: Vec<f32>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas {
            width,
            height,
            rgb: vec![[0, 0, 0]; width * height],
            alpha: vec![0.0; width * height],
        }
    }

    fn set(&mut self, x: usize, y: usize, rgb: [u8; 3], a: f32) {
        let i = y * self.width + x;
        self.rgb[i] = rgb;
        self.alpha[i] = a.clamp(0.0, 1.0);
    }

    /// Rounded rectangle filling the canvas (for the progress pill).
    pub fn rounded_rect(
        width: usize,
        height: usize,
        radius: f32,
        rgb: [u8; 3],
        alpha: f32,
    ) -> Canvas {
        let mut c = Canvas::new(width, height);
        let (w, h) = (width as f32, height as f32);
        let r = radius.min(w / 2.0).min(h / 2.0);
        for y in 0..height {
            for x in 0..width {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let qx = (px - w / 2.0).abs() - (w / 2.0 - r);
                let qy = (py - h / 2.0).abs() - (h / 2.0 - r);
                let outside =
                    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                c.set(x, y, rgb, cover(-outside) * alpha);
            }
        }
        c
    }

    /// Fill a rectangle with a colour, keeping the existing alpha.
    pub fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, rgb: [u8; 3]) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                self.rgb[y * self.width + x] = rgb;
            }
        }
    }

    /// Premultiplied BGRA, the layout Windows layered windows expect.
    /// `rgb` may differ from `self.rgb` (e.g. after text was drawn on it).
    pub fn to_premultiplied_bgra(&self, rgb: &[[u8; 3]]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.alpha.len() * 4);
        for (px, &a) in rgb.iter().zip(&self.alpha) {
            let pm = |c: u8| (f32::from(c) * a + 0.5) as u8;
            out.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), (a * 255.0 + 0.5) as u8]);
        }
        out
    }

    /// Every covered pixel at the minimum non-zero alpha: invisible, but still
    /// hit-testable, so OLE drag-and-drop can find the window.
    pub fn probe(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.alpha.len() * 4);
        for &a in &self.alpha {
            out.extend_from_slice(&[0, 0, 0, if a > 0.0 { 1 } else { 0 }]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Format::*;

    #[test]
    fn convert_items_follow_routes() {
        let it = items(Mode::Convert, &[Png], false);
        assert_eq!(it[0].action, Action::Convert(Jpeg));
        assert!(it.iter().all(|i| i.action != Action::Convert(Png)));
        assert!(it.len() <= MAX_SEGMENTS);
    }

    #[test]
    fn tool_items() {
        let labels: Vec<String> = items(Mode::Tools, &[Pdf, Pdf], false)
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert_eq!(labels, ["Merge", "Split", "SHA-256"]);
    }

    #[test]
    fn hit_testing_matches_layout() {
        let g = Geometry::for_scale(1.0);
        let n = 8;
        assert_eq!(g.hit(160.0, 160.0, n), Hit::Center);
        assert_eq!(g.hit(1.0, 1.0, n), Hit::Outside);
        assert_eq!(
            g.hit(160.0, 20.0, n),
            Hit::Segment(0),
            "straight up is the first segment"
        );
        assert_eq!(
            g.hit(300.0, 160.0, n),
            Hit::Segment(2),
            "3 o'clock is a quarter turn"
        );
        for i in 0..n {
            let (x, y) = g.label_point(i, n);
            assert_eq!(
                g.hit(x, y, n),
                Hit::Segment(i),
                "label {i} sits in its segment"
            );
        }
    }

    #[test]
    fn render_highlights_hovered_segment_only() {
        let g = Geometry::for_scale(0.5);
        let c = g.render(4, Some(1), &Theme::DARK);
        let at = |(x, y): (f32, f32)| c.rgb[y as usize * c.width + x as usize];
        assert_eq!(at(g.label_point(1, 4)), Theme::DARK.highlight);
        assert_eq!(at(g.label_point(0, 4)), Theme::DARK.ring);
        assert_eq!(c.alpha[0], 0.0, "corners are transparent");
        let bgra = c.to_premultiplied_bgra(&c.rgb);
        assert_eq!(bgra.len(), c.width * c.height * 4);
        assert!(c.probe().chunks(4).any(|p| p[3] == 1));
        // No hit-test holes anywhere inside the outer radius.
        let ctr = g.size as f32 / 2.0;
        for y in 0..c.height {
            for x in 0..c.width {
                let (dx, dy) = (x as f32 + 0.5 - ctr, y as f32 + 0.5 - ctr);
                if (dx * dx + dy * dy).sqrt() < g.r_outer - 1.0 {
                    assert!(c.alpha[y * c.width + x] > 0.0, "hole at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn pill_has_soft_corners() {
        let c = Canvas::rounded_rect(100, 40, 20.0, [0, 0, 0], 1.0);
        assert_eq!(c.alpha[0], 0.0);
        assert_eq!(c.alpha[20 * 100 + 50], 1.0);
    }
}
