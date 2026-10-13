//! 本体のパネルに寄せた部品。ノブ (QUICK ACCESS の V1-V3 に相当)、
//! 白バックライトの LCD 風の枠、反転表示の見出し、レベルメータ。

use egui::{Align2, Color32, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};
use std::f32::consts::PI;

// ---- 配色 -----------------------------------------------------------------
//
// 本体は銀色の筐体に白バックライトの LCD (Owner's Manual p.11 "the white
// backlighting of the LCD")。画面全体は暗めの筐体色にして、LCD の部分だけ
// 明るくする。

pub const PANEL: Color32 = Color32::from_rgb(0x26, 0x28, 0x2c);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x32, 0x35, 0x3a);
pub const PANEL_LINE: Color32 = Color32::from_rgb(0x44, 0x48, 0x4f);
pub const SILK: Color32 = Color32::from_rgb(0xc8, 0xcc, 0xd2);
pub const SILK_DIM: Color32 = Color32::from_rgb(0x8a, 0x90, 0x98);
pub const ACCENT: Color32 = Color32::from_rgb(0x5a, 0xb4, 0xff);
pub const LCD_BG: Color32 = Color32::from_rgb(0xdc, 0xe6, 0xec);
pub const LCD_INK: Color32 = Color32::from_rgb(0x16, 0x22, 0x2c);
pub const LCD_DIM: Color32 = Color32::from_rgb(0x6c, 0x7c, 0x88);
pub const MUTE: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x4c);
pub const SOLO: Color32 = Color32::from_rgb(0xe8, 0xc4, 0x4a);
pub const OK: Color32 = Color32::from_rgb(0x6c, 0xd0, 0x8c);

/// LCD 風の枠。中は暗い文字。
pub fn lcd<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(LCD_BG)
        .stroke(Stroke::new(2.0_f32, Color32::from_rgb(0x10, 0x12, 0x14)))
        .rounding(3.0)
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            // strong() の文字は override_text_color ではなく widgets の色を使う。
            let v = ui.visuals_mut();
            v.override_text_color = Some(LCD_INK);
            v.widgets.noninteractive.fg_stroke.color = LCD_INK;
            v.widgets.active.fg_stroke.color = LCD_INK;
            add(ui)
        })
        .inner
}

/// LCD の反転表示の見出し ("PART PARAMETER" など)。
pub fn inverted(ui: &mut Ui, text: &str) {
    let font = FontId::monospace(13.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, LCD_BG);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y + 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 1.0, LCD_INK);
    ui.painter().galley(rect.min + vec2(6.0, 2.0), galley, LCD_BG);
}

/// 筐体に印刷された文字ふうの見出し。
pub fn silk(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).color(SILK_DIM).size(11.0).strong());
}

/// 縦ドラッグで回すノブ。ダブルクリックで既定値、ホイールで 1 ずつ。
///
/// 値は生の値 (`min..=max`)。表示は `fmt` が作る。
pub fn knob(
    ui: &mut Ui,
    label: &str,
    value: &mut u8,
    min: u8,
    max: u8,
    default: u8,
    fmt: &dyn Fn(u8) -> String,
) -> Response {
    let size = vec2(62.0, 78.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let range = (max - min) as f32;
    let old = *value;

    if resp.dragged() {
        let id = resp.id.with("acc");
        let fine = ui.input(|i| i.modifiers.shift);
        let per_px = range / if fine { 600.0 } else { 150.0 };
        let mut acc: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
        acc -= resp.drag_delta().y * per_px;
        let step = acc.trunc();
        acc -= step;
        ui.data_mut(|d| d.insert_temp(id, acc));
        *value = (*value as f32 + step).clamp(min as f32, max as f32) as u8;
    }
    if resp.double_clicked() {
        *value = default;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let d = if scroll > 0.0 { 1i32 } else { -1 };
            *value = (*value as i32 + d).clamp(min as i32, max as i32) as u8;
        }
    }
    if *value != old {
        resp.mark_changed();
    }

    let p = ui.painter();
    let c = pos2(rect.center().x, rect.top() + 24.0);
    let r = 17.0;
    // 目盛りの弧 (7 時から 5 時まで)。
    let a0 = PI * 0.75;
    let sweep = PI * 1.5;
    let t = if range > 0.0 { (*value - min) as f32 / range } else { 0.0 };
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = 24;
        (0..=n)
            .map(|k| {
                let a = from + (to - from) * k as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * (r + 4.0)
            })
            .collect()
    };
    p.add(Shape::line(arc(a0, a0 + sweep), Stroke::new(2.5_f32, PANEL_LINE)));
    // 中央が既定のパラメータ (-64..+63 等) は中央から、それ以外は左端から塗る。
    let from_t = if default > min && default < max { (default - min) as f32 / range } else { 0.0 };
    let (s, e) = if t >= from_t { (from_t, t) } else { (t, from_t) };
    if e > s {
        p.add(Shape::line(arc(a0 + sweep * s, a0 + sweep * e), Stroke::new(2.5_f32, ACCENT)));
    }
    let hover = resp.hovered() || resp.dragged();
    p.circle(c, r, if hover { PANEL_HI } else { Color32::from_rgb(0x1c, 0x1d, 0x20) }, Stroke::new(1.0_f32, PANEL_LINE));
    let a = a0 + sweep * t;
    p.line_segment([c + vec2(a.cos(), a.sin()) * 6.0, c + vec2(a.cos(), a.sin()) * (r - 2.0)], Stroke::new(2.0_f32, SILK));

    p.text(
        pos2(rect.center().x, rect.top() + 50.0),
        Align2::CENTER_CENTER,
        fmt(*value),
        FontId::monospace(12.0),
        if hover { Color32::WHITE } else { SILK },
    );
    p.text(
        pos2(rect.center().x, rect.top() + 66.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(10.5),
        SILK_DIM,
    );
    resp
}

/// ステレオのピークメータ (横向き)。`peak` は線形の振幅。
pub fn meter(ui: &mut Ui, peak: [f32; 2], width: f32) {
    let h = 7.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h * 2.0 + 3.0), Sense::hover());
    let p = ui.painter();
    for (k, v) in peak.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(0.0, k as f32 * (h + 3.0)), vec2(width, h));
        p.rect_filled(r, 1.0, Color32::from_rgb(0x14, 0x15, 0x17));
        let db = 20.0 * v.max(1e-6).log10();
        // -48dB..0dB を 0..1 に。
        let t = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
        if t > 0.0 {
            let fill = Rect::from_min_size(r.min, vec2(width * t, h));
            let col = if db > -1.0 {
                MUTE
            } else if db > -9.0 {
                SOLO
            } else {
                OK
            };
            p.rect_filled(fill, 1.0, col);
        }
    }
}

/// パート一覧の小さなレベルバー。
pub fn level_bar(ui: &mut Ui, level: u8, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(44.0, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 1.0, Color32::from_rgb(0x14, 0x15, 0x17));
    let w = rect.width() * level as f32 / 127.0;
    p.rect_filled(
        Rect::from_min_size(rect.min, vec2(w, rect.height())),
        1.0,
        if active { ACCENT } else { SILK_DIM },
    );
}

/// 小さな四角いトグル (M / S)。
pub fn toggle_box(ui: &mut Ui, on: &mut bool, text: &str, color: Color32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = ui.painter();
    let fill = if *on { color } else { Color32::from_rgb(0x1c, 0x1d, 0x20) };
    p.rect(rect.shrink(1.0), 2.0, fill, Stroke::new(1.0_f32, PANEL_LINE));
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        if *on { Color32::BLACK } else { SILK_DIM },
    );
    resp
}

// ---- 値の表記 -------------------------------------------------------------

/// 64 を 0 とする符号付き (-64 - +63)。
pub fn signed64(v: u8) -> String {
    let d = v as i32 - 64;
    if d > 0 { format!("+{d}") } else { format!("{d}") }
}

pub fn plain(v: u8) -> String {
    format!("{v}")
}

/// L64 - 0 - 63R。
pub fn pan(v: u8) -> String {
    let d = v as i32 - 64;
    match d {
        0 => "0".into(),
        d if d < 0 => format!("L{}", -d),
        d => format!("{d}R"),
    }
}

/// ノート番号を C-1 - G9 で。
pub fn note(v: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[v as usize % 12], v as i32 / 12 - 1)
}
