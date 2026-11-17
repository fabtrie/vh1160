//! A live line chart in the look of the Manson app's uPlot charts.

use gpui::{
    App, Bounds, ContentMask, Hsla, IntoElement, PathBuilder, Pixels, SharedString, Styled,
    TextAlign, TextRun, Window, canvas, fill, font, point, px, size,
};

use crate::theme::{Palette, UI_FONT};

pub struct Series {
    pub color: Hsla,
    /// (time in s, value)
    pub points: Vec<(f64, f64)>,
}

#[derive(Clone, Copy)]
pub struct Axis {
    /// Time of the right edge.
    pub t_end: f64,
    pub window_s: f64,
    pub y_top: f64,
    /// Log scale from 0.1 to `y_top`.
    pub log: bool,
}

/// Smallest value ≥ x of the form {1, 2, 2.5, 5} × 10ⁿ.
pub fn nice_ceil(x: f64) -> f64 {
    let exp = 10f64.powf(x.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * exp >= x - 1e-12 {
            return m * exp;
        }
    }
    10.0 * exp
}

/// Round top for the y-axis with hysteresis: grows as soon as the data needs
/// more room, shrinks only once the data uses less than 40 % of it.
pub fn y_top(current: f64, data_max: f64, min_top: f64) -> f64 {
    let target = nice_ceil((data_max * 1.1).max(min_top));
    if target > current || target < current * 0.4 {
        target
    } else {
        current
    }
}

const LOG_MIN: f64 = 0.1;
const LEFT: f32 = 48.0;
const RIGHT: f32 = 8.0;
const TOP: f32 = 6.0;
const BOTTOM: f32 = 22.0;

pub fn chart(series: Vec<Series>, axis: Axis, p: &Palette) -> impl IntoElement {
    let (grid, label_color) = (p.grid, p.axis);
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window: &mut Window, cx: &mut App| {
            let w = f32::from(b.size.width) - LEFT - RIGHT;
            let h = f32::from(b.size.height) - TOP - BOTTOM;
            if w <= 10.0 || h <= 10.0 {
                return;
            }
            let ox = f32::from(b.origin.x) + LEFT;
            let oy = f32::from(b.origin.y) + TOP;
            let t0 = axis.t_end - axis.window_s;
            let x_of = |t: f64| ox + ((t - t0) / axis.window_s) as f32 * w;
            let top = axis.y_top.max(if axis.log { LOG_MIN * 10.0 } else { 1e-9 });
            let y_of = |v: f64| {
                let f = if axis.log {
                    (v.max(LOG_MIN).log10() - LOG_MIN.log10()) / (top.log10() - LOG_MIN.log10())
                } else {
                    v / top
                };
                oy + h - (f.clamp(-0.02, 1.02) as f32) * h
            };

            // Horizontal grid lines and y labels.
            let ticks: Vec<f64> = if axis.log {
                let decades = (top / LOG_MIN).log10().round() as i32;
                (0..=decades).map(|k| LOG_MIN * 10f64.powi(k)).collect()
            } else {
                // Round steps (top 5 → 1, top 20 → 5, top 500 → 100), like uPlot.
                let step = nice_ceil(top / 5.0);
                (0..)
                    .map(|k| f64::from(k) * step)
                    .take_while(|v| *v <= top * 1.000_001)
                    .collect()
            };
            for v in ticks {
                let y = y_of(v).round();
                window.paint_quad(fill(
                    Bounds::new(point(px(ox), px(y)), size(px(w), px(1.0))),
                    grid,
                ));
                label(
                    &fmt_tick(v, top),
                    ox - 6.0,
                    y - 7.0,
                    TextAlign::Right,
                    label_color,
                    window,
                    cx,
                );
            }
            // Time labels every window/6.
            for k in 0..=6 {
                let t = t0 + axis.window_s * f64::from(k) / 6.0;
                let text = format!("{} s", (t - axis.t_end).round() as i64);
                label(
                    &text,
                    x_of(t),
                    oy + h + 5.0,
                    TextAlign::Center,
                    label_color,
                    window,
                    cx,
                );
            }

            let plot = Bounds::new(point(px(ox), px(oy - 2.0)), size(px(w), px(h + 4.0)));
            window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
                for s in &series {
                    let mut pts = s.points.iter().filter(|(t, _)| *t >= t0 - 1.0);
                    let Some(&(t, v)) = pts.next() else { continue };
                    let mut pb = PathBuilder::stroke(px(1.5));
                    pb.move_to(point(px(x_of(t)), px(y_of(v))));
                    for &(t, v) in pts {
                        pb.line_to(point(px(x_of(t)), px(y_of(v))));
                    }
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, s.color);
                    }
                }
            });
        },
    )
    .w_full()
    .h(px(170.0))
}

/// Up to two decimals, without trailing zeros (0.25, 0.5, 2, 100).
fn fmt_tick(v: f64, _top: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Paints one line of axis text; `x` is the left, centre or right edge per `align`.
fn label(
    text: &str,
    x: f32,
    y: f32,
    align: TextAlign,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let run = TextRun {
        len: text.len(),
        font: font(UI_FONT),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(11.0),
        &[run],
        None,
    );
    let width = f32::from(line.width);
    let left = match align {
        TextAlign::Left => x,
        TextAlign::Center => x - width / 2.0,
        TextAlign::Right => x - width,
    };
    let _ = line.paint(
        point(px(left), px(y)),
        px(14.0),
        TextAlign::Left,
        None,
        window,
        cx,
    );
}
