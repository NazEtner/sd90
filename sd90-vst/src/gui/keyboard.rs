//! 画面の鍵盤。選んでいるパートのチャンネルにノートを送る。
//!
//! 送るのはエディタの他の操作と同じく `Control` の送信待ち経由 (= `process`
//! 経由)。DAW のノートと同じ道を通るので、追い越したり塞いだりしない。
//!
//! * マウス: 押すと鳴り、押したまま横に滑らせるとグリッサンド
//! * PC のキーボード: Z S X D C V G B H N J M , が下の段、
//!   Q 2 W 3 E R 5 T 6 Y 7 U I が 1 オクターブ上 (数値を打ち込んでいる間は効かない)
//! * HOLD は CC#64 (Hold 1)、ベンドは離すと中央に戻る
//!
//! 鳴らしたままにしないこと。送り先のパートが変わったとき、鍵盤を閉じたとき、
//! ウィンドウを閉じたときは押さえているノートを全部離す (`release_all`)。

use super::widgets as w;
use crate::sd90::Out;
use egui::{Color32, Key, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

/// 見える白鍵の数 (5 オクターブ + 1)。
const WHITES: usize = 36;
/// 白鍵 7 つの中の、右に黒鍵があるもの (C D F G A)。
const HAS_BLACK: [bool; 7] = [true, true, false, true, true, true, false];
const WHITE_SEMI: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

/// PC のキーと、表示の左端の C からの半音数。
const PC_KEYS: [(Key, u8); 30] = [
    (Key::Z, 0), (Key::S, 1), (Key::X, 2), (Key::D, 3), (Key::C, 4), (Key::V, 5),
    (Key::G, 6), (Key::B, 7), (Key::H, 8), (Key::N, 9), (Key::J, 10), (Key::M, 11),
    (Key::Comma, 12),
    (Key::Q, 12), (Key::Num2, 13), (Key::W, 14), (Key::Num3, 15), (Key::E, 16), (Key::R, 17),
    (Key::Num5, 18), (Key::T, 19), (Key::Num6, 20), (Key::Y, 21), (Key::Num7, 22), (Key::U, 23),
    (Key::I, 24), (Key::Num9, 25), (Key::O, 26), (Key::Num0, 27), (Key::P, 28),
];

/// 送り先。Port (cable) と MIDI チャンネル。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub cable: u8,
    pub ch: u8,
}

pub struct Keyboard {
    /// 表示の左端の C のオクターブ (C2 = 36 なら 2)。
    octave: i32,
    velocity: u8,
    hold: bool,
    /// ベンド -1.0 .. 1.0。
    bend: f32,
    /// マウスで押さえているノート。
    mouse_note: Option<u8>,
    /// PC のキーで押さえているノート。
    key_notes: Vec<(Key, u8)>,
    /// 鳴らしたときの送り先。途中でパートを替えても、ここに離しを送る。
    sounding: Vec<(Target, u8)>,
    target: Option<Target>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            octave: 2,
            velocity: 100,
            hold: false,
            bend: 0.0,
            mouse_note: None,
            key_notes: Vec::new(),
            sounding: Vec::new(),
            target: None,
        }
    }
}

impl Keyboard {
    fn base(&self) -> i32 {
        (self.octave + 1) * 12
    }

    fn note_on(&mut self, t: Target, note: u8, out: &mut Vec<Out>) {
        out.push(Out::msg(t.cable, &[0x90 | t.ch, note, self.velocity.max(1)]));
        self.sounding.push((t, note));
    }

    fn note_off(&mut self, note: u8, out: &mut Vec<Out>) {
        if let Some(i) = self.sounding.iter().position(|&(_, n)| n == note) {
            let (t, n) = self.sounding.remove(i);
            out.push(Out::msg(t.cable, &[0x80 | t.ch, n, 0]));
        }
    }

    /// 押さえているものを全部離す。HOLD とベンドも戻す。
    pub fn release_all(&mut self, out: &mut Vec<Out>) {
        for (t, n) in std::mem::take(&mut self.sounding) {
            out.push(Out::msg(t.cable, &[0x80 | t.ch, n, 0]));
        }
        self.mouse_note = None;
        self.key_notes.clear();
        if let Some(t) = self.target {
            if self.hold {
                out.push(Out::msg(t.cable, &[0xB0 | t.ch, 64, 0]));
            }
            if self.bend != 0.0 {
                out.push(Out::msg(t.cable, &[0xE0 | t.ch, 0x00, 0x40]));
            }
        }
        self.hold = false;
        self.bend = 0.0;
    }

    /// 鍵盤の段を描き、送るものを `out` に積む。`target` が `None` なら鳴らさない
    /// (受信 OFF のパートなど)。
    pub fn ui(&mut self, ui: &mut Ui, target: Option<Target>, label: &str, out: &mut Vec<Out>) {
        if target != self.target {
            self.release_all(out);
            self.target = target;
        }

        ui.horizontal(|ui| {
            // ---- 左の操作子 ----
            ui.vertical(|ui| {
                ui.set_width(150.0);
                ui.label(RichText::new(format!("KEYBOARD  →  {label}")).color(w::SILK_DIM).size(11.0).strong());
                ui.horizontal(|ui| {
                    if ui.small_button("OCT -").clicked() && self.octave > -1 {
                        self.release_all(out);
                        self.octave -= 1;
                    }
                    ui.label(RichText::new(format!("C{}", self.octave)).monospace().color(w::SILK));
                    if ui.small_button("OCT +").clicked() && self.octave < 5 {
                        self.release_all(out);
                        self.octave += 1;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Vel").color(w::SILK_DIM).size(11.0));
                    ui.add(egui::Slider::new(&mut self.velocity, 1..=127).show_value(true));
                });
                let mut hold = self.hold;
                if ui
                    .add(egui::SelectableLabel::new(hold, "HOLD"))
                    .on_hover_text("Hold 1 (CC#64)")
                    .clicked()
                {
                    hold = !hold;
                }
                if hold != self.hold {
                    self.hold = hold;
                    if let Some(t) = target {
                        out.push(Out::msg(t.cable, &[0xB0 | t.ch, 64, if hold { 127 } else { 0 }]));
                    }
                }
            });

            // ---- ピッチベンド (離すと中央へ) ----
            let (brect, bresp) = ui.allocate_exact_size(vec2(22.0, 96.0), Sense::drag());
            let old = self.bend;
            if bresp.dragged() {
                if let Some(p) = bresp.interact_pointer_pos() {
                    self.bend = (1.0 - 2.0 * (p.y - brect.top()) / brect.height()).clamp(-1.0, 1.0);
                }
            } else {
                self.bend = 0.0;
            }
            if self.bend != old
                && let Some(t) = target
            {
                let v = ((self.bend + 1.0) * 8191.5).round().clamp(0.0, 16383.0) as u16;
                out.push(Out::msg(t.cable, &[0xE0 | t.ch, (v & 0x7F) as u8, (v >> 7) as u8]));
            }
            let p = ui.painter();
            p.rect(brect, 3.0, Color32::from_rgb(0x1c, 0x1d, 0x20), Stroke::new(1.0_f32, w::PANEL_LINE));
            let y = brect.center().y - self.bend * brect.height() / 2.0;
            p.rect_filled(Rect::from_center_size(pos2(brect.center().x, y), vec2(18.0, 10.0)), 2.0, w::SILK);
            bresp.on_hover_text("Pitch Bend (離すと中央に戻る)");
            ui.add_space(6.0);

            // ---- 鍵盤 ----
            self.keys(ui, target, out);
        });

        // ---- PC のキー ----
        // 数値欄に打ち込んでいる間は鍵盤にしない。
        if ui.ctx().memory(|m| m.focused().is_none()) {
            let events = ui.input(|i| i.events.clone());
            for e in events {
                if let egui::Event::Key { key, pressed, repeat, modifiers, .. } = e {
                    if repeat || modifiers.ctrl || modifiers.alt {
                        continue;
                    }
                    let Some(&(_, semi)) = PC_KEYS.iter().find(|(k, _)| *k == key) else {
                        continue;
                    };
                    let note = self.base() + semi as i32;
                    if !(0..=127).contains(&note) {
                        continue;
                    }
                    let note = note as u8;
                    if pressed {
                        if let Some(t) = target
                            && !self.key_notes.iter().any(|(k, _)| *k == key)
                        {
                            self.key_notes.push((key, note));
                            self.note_on(t, note, out);
                        }
                    } else if let Some(i) = self.key_notes.iter().position(|(k, _)| *k == key) {
                        let (_, n) = self.key_notes.remove(i);
                        self.note_off(n, out);
                    }
                }
            }
        }
    }

    fn keys(&mut self, ui: &mut Ui, target: Option<Target>, out: &mut Vec<Out>) {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 96.0), Sense::click_and_drag());
        let ww = rect.width() / WHITES as f32;
        let bw = ww * 0.62;
        let bh = rect.height() * 0.6;
        let base = self.base();

        // 白鍵 / 黒鍵の矩形とノート番号。
        let mut whites = Vec::with_capacity(WHITES);
        let mut blacks = Vec::new();
        for i in 0..WHITES {
            let oct = (i / 7) as i32;
            let k = i % 7;
            let note = base + oct * 12 + WHITE_SEMI[k] as i32;
            let r = Rect::from_min_size(pos2(rect.left() + i as f32 * ww, rect.top()), vec2(ww, rect.height()));
            whites.push((r, note));
            if HAS_BLACK[k] && i + 1 < WHITES {
                let br = Rect::from_min_size(pos2(r.right() - bw / 2.0, rect.top()), vec2(bw, bh));
                blacks.push((br, note + 1));
            }
        }
        let hit = |p: Pos2| -> Option<u8> {
            blacks
                .iter()
                .chain(whites.iter())
                .find(|(r, _)| r.contains(p))
                .and_then(|&(_, n)| u8::try_from(n).ok().filter(|n| *n < 128))
        };

        // マウス。押している間はポインタの下の鍵を鳴らす (滑らせるとグリッサンド)。
        let now = if resp.is_pointer_button_down_on() {
            ui.input(|i| i.pointer.interact_pos()).and_then(hit)
        } else {
            None
        };
        if now != self.mouse_note {
            if let Some(n) = self.mouse_note.take() {
                self.note_off(n, out);
            }
            if let (Some(n), Some(t)) = (now, target) {
                self.note_on(t, n, out);
                self.mouse_note = Some(n);
            }
        }

        let down = |n: i32| self.sounding.iter().any(|&(_, s)| s as i32 == n);
        let p = ui.painter_at(rect);
        for &(r, n) in &whites {
            let fill = if down(n) { w::ACCENT } else if target.is_some() { Color32::from_rgb(0xee, 0xef, 0xf1) } else { Color32::from_rgb(0x9a, 0x9c, 0xa0) };
            p.rect(r.shrink2(vec2(0.5, 0.0)), 2.0, fill, Stroke::new(1.0_f32, Color32::from_rgb(0x30, 0x32, 0x36)));
            if n % 12 == 0 {
                p.text(
                    pos2(r.center().x, r.bottom() - 9.0),
                    egui::Align2::CENTER_CENTER,
                    w::note(n as u8),
                    egui::FontId::proportional(9.5),
                    Color32::from_rgb(0x50, 0x54, 0x5a),
                );
            }
        }
        for &(r, n) in &blacks {
            let fill = if down(n) { w::ACCENT } else { Color32::from_rgb(0x18, 0x19, 0x1b) };
            p.rect(r, 2.0, fill, Stroke::new(1.0_f32, Color32::BLACK));
        }
        if target.is_none() {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "このパートは受信 OFF (Rx CH)",
                egui::FontId::proportional(13.0),
                w::MUTE,
            );
        }
    }
}
