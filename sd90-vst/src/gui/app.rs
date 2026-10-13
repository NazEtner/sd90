//! エディタの中身。構成は本体のパネルと Owner's Manual の章立てに合わせる。
//!
//! * 上段 … 音源モード (INST Init, p.20 / p.77)、接続状態、出力メータ
//! * 左 … 32 パートの一覧。LCD のパート表示と同じく A01-A16 / B01-B16、
//!   M / S はミュート / ソロ (p.26-27)
//! * 右のタブ … パネルのボタンに対応
//!   - INST/DRUM SET: パートパラメータ (p.21-37)
//!   - EFFECTS: 音源のエフェクト (p.38-47)
//!   - ALL: 全パート共通 (p.31, p.37)
//!   - SYSTEM: Device ID (p.72)、一括送信など
//!
//! 編集は手元の `Model` の写しに対して行い、フレームの終わりに前の写しとの
//! 差分を取って、変わったところだけを送る。

use super::keyboard::{Keyboard, Target};
use super::widgets::{self as w, knob};
use crate::control::Control;
use crate::sd90::{self, Mode, Model, OUT_A, OUT_MFX, OUT_PATCH, Out, PARTS, Part};
use crate::tones::{self, SoundSet};
use egui::{Color32, RichText, Ui, vec2};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Part,
    Effects,
    All,
    System,
}

pub struct App {
    ctrl: Arc<Control>,
    m: Model,
    rev: u32,
    tab: Tab,
    sel: usize,
    /// 確認待ちのモード切り替え。
    confirm: Option<Mode>,
    /// 次のフレームの頭で実行するモード切り替え。
    switch_to: Option<Mode>,
    /// PREVIEW で鳴らした音を止める時刻とメッセージ。
    preview_off: Option<(Instant, Out)>,
    keyboard: Keyboard,
    show_keyboard: bool,
    eq_link: bool,
    peak: [f32; 2],
    /// レート測定の前回値 (時刻, ホストの累計, 取り込みの累計) と表示文字列。
    rate_snap: Option<(Instant, u64, u64)>,
    rate_text: String,
    /// ホストの実レートが 48kHz から外れている。
    rate_warn: bool,
}

impl App {
    pub fn new(ctrl: Arc<Control>) -> App {
        let m = ctrl.model.lock().map(|m| m.clone()).unwrap_or_default();
        let rev = ctrl.model_rev.load(Relaxed);
        App {
            ctrl,
            eq_link: m.common.eq[0] == m.common.eq[1],
            m,
            rev,
            tab: Tab::Part,
            sel: 0,
            confirm: None,
            switch_to: None,
            preview_off: None,
            rate_snap: None,
            rate_text: String::new(),
            rate_warn: false,
            peak: [0.0; 2],
            keyboard: Keyboard::default(),
            show_keyboard: super::keyboard_by_default(),
        }
    }

    /// フォントと配色。ウィンドウを作った直後に 1 回。
    pub fn setup(&self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        // ステータスやツールチップに日本語が出るので、システムの日本語フォントを
        // 後ろに足す。見つからなければ英字だけで動く。
        if let Some(data) = japanese_font() {
            fonts
                .font_data
                .insert("jp".into(), egui::FontData::from_static(data));
            // 和文フォントを先頭にすると英字の行高まで変わってレイアウトが崩れる。
            // 後ろに足して、和文の字形だけ借りる。
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&fam) {
                    list.push("jp".into());
                }
            }
        }
        ctx.set_fonts(fonts);

        let mut v = egui::Visuals::dark();
        v.panel_fill = w::PANEL;
        v.window_fill = w::PANEL_HI;
        v.extreme_bg_color = Color32::from_rgb(0x1a, 0x1b, 0x1e);
        v.faint_bg_color = w::PANEL_HI;
        v.selection.bg_fill = Color32::from_rgb(0x2c, 0x5a, 0x86);
        v.selection.stroke.color = w::ACCENT;
        v.widgets.noninteractive.fg_stroke.color = w::SILK;
        v.widgets.noninteractive.bg_stroke.color = w::PANEL_LINE;
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x1c, 0x1d, 0x20);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x1c, 0x1d, 0x20);
        v.widgets.hovered.weak_bg_fill = w::PANEL_HI;
        ctx.set_visuals(v);
        ctx.style_mut(|s| {
            s.spacing.item_spacing = vec2(6.0, 4.0);
            s.spacing.button_padding = vec2(8.0, 3.0);
            s.interaction.tooltip_delay = 0.4;
        });
    }

    /// ウィンドウを閉じる前。鳴らしっぱなしにしない。
    pub fn closing(&mut self) {
        if let Some((_, off)) = self.preview_off.take() {
            self.ctrl.post([off]);
        }
        let mut v = Vec::new();
        self.keyboard.release_all(&mut v);
        self.ctrl.post(v);
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        // エディタの外で状態が変わった (setState / DAW からの CC)。
        let rev = self.ctrl.model_rev.load(Relaxed);
        if rev != self.rev {
            self.rev = rev;
            if let Ok(m) = self.ctrl.model.lock() {
                self.m = m.clone();
            }
        }
        if let Some(mode) = self.switch_to.take() {
            self.m.switch_mode(mode);
            self.ctrl.post([mode.reset(self.m.device_id), Out::Wait(300)]);
            self.commit_model();
        }
        if let Some((at, off)) = self.preview_off
            && Instant::now() >= at
        {
            self.ctrl.post([off]);
            self.preview_off = None;
        }
        let pk = self.ctrl.take_peak();
        for k in 0..2 {
            // 落ちるのはゆっくり。
            self.peak[k] = pk[k].max(self.peak[k] * 0.82);
        }

        let before = self.m.clone();

        egui::TopBottomPanel::top("top")
            .exact_height(58.0)
            .frame(egui::Frame::none().fill(Color32::from_rgb(0x1e, 0x20, 0x23)).inner_margin(egui::Margin::symmetric(12.0, 8.0)))
            .show(ctx, |ui| self.top(ui));

        // 鍵盤は左のパート一覧も含めた全幅に置く。下の段は SidePanel より先に足す。
        if self.show_keyboard {
            egui::TopBottomPanel::bottom("keyboard")
                .exact_height(112.0)
                .frame(egui::Frame::none().fill(Color32::from_rgb(0x1e, 0x20, 0x23)).inner_margin(egui::Margin::symmetric(10.0, 7.0)))
                .show(ctx, |ui| {
                    let p = &self.m.parts[self.sel];
                    // 送り先は選んでいるパート。受信 OFF なら鳴らせない。
                    let target = p.rx_sw.then(|| Target { cable: Part::cable(self.sel), ch: p.rx_ch & 0x0F });
                    let label = match target {
                        Some(t) => format!("{} (ch {})", Part::label(self.sel), t.ch + 1),
                        None => Part::label(self.sel),
                    };
                    let mut out = Vec::new();
                    self.keyboard.ui(ui, target, &label, &mut out);
                    self.ctrl.post(out);
                });
        }

        egui::SidePanel::left("parts")
            .exact_width(430.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(w::PANEL).inner_margin(egui::Margin::same(8.0)))
            .show(ctx, |ui| self.part_list(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(w::PANEL).inner_margin(egui::Margin::same(10.0)))
            .show(ctx, |ui| {
                self.tabs(ui);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.tab {
                    Tab::Part => self.part_page(ui),
                    Tab::Effects => self.effects_page(ui),
                    Tab::All => self.all_page(ui),
                    Tab::System => self.system_page(ui),
                });
            });

        self.confirm_window(ctx);

        // 差分を送る。
        if self.m.mode == before.mode {
            let changes = before.diff(&self.m);
            if !changes.is_empty() {
                self.m.recall = true;
                for c in &changes {
                    self.ctrl.send_change(&self.m, *c);
                }
                self.commit_model();
            }
        }
    }

    fn commit_model(&mut self) {
        if let Ok(mut m) = self.ctrl.model.lock() {
            *m = self.m.clone();
        }
    }

    // ---- 上段 -------------------------------------------------------------

    fn top(&mut self, ui: &mut Ui) {
        // 幅を決め打ちで並べる。入れ子のレイアウトに任せると、右寄せの中の
        // 縦並びが幅 0 で潰れて 1 文字ずつ折り返す。
        let block = |ui: &mut Ui, width: f32, add: &mut dyn FnMut(&mut Ui)| {
            ui.allocate_ui_with_layout(vec2(width, 42.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(width);
                add(ui)
            });
        };
        ui.horizontal(|ui| {
            block(ui, 210.0, &mut |ui| {
                ui.label(RichText::new("EDIROL").color(w::SILK_DIM).size(11.0).strong().italics());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SD-90").color(Color32::WHITE).size(22.0).strong().italics());
                    ui.label(RichText::new("STUDIO Canvas").color(w::SILK).size(15.0).italics());
                });
            });

            // 音源モード。LCD の右端の USB / GM2 / GS 表示に相当。
            block(ui, 280.0, &mut |ui| {
                w::silk(ui, "SOUND GENERATOR MODE");
                ui.horizontal(|ui| {
                    for mode in Mode::ALL {
                        let on = self.m.mode == mode;
                        let b = egui::Button::new(RichText::new(mode.name()).color(if on { Color32::BLACK } else { w::SILK }))
                            .fill(if on { w::ACCENT } else { Color32::from_rgb(0x1c, 0x1d, 0x20) })
                            .min_size(vec2(62.0, 20.0));
                        if ui
                            .add(b)
                            .on_hover_text("音源モードを切り替える。音源は初期化される (Owner's Manual p.20, p.77)")
                            .clicked()
                            && !on
                        {
                            self.confirm = Some(mode);
                        }
                    }
                });
            });

            let rest = ui.available_width() - 200.0 - 170.0 - 80.0;
            ui.add_space(rest.max(0.0));

            block(ui, 200.0, &mut |ui| {
                let online = self.ctrl.online.load(Relaxed);
                w::silk(ui, "USB");
                if online {
                    let lat = self.ctrl.latency.load(Relaxed);
                    let hz = self.ctrl.host_rate.load(Relaxed).max(1) as f32;
                    ui.label(RichText::new(format!("● ONLINE   latency {:.2} ms", lat as f32 * 1000.0 / hz)).color(w::OK).monospace())
                        .on_hover_text(format!("ホストに申告しているレイテンシ {lat} フレーム"));
                } else {
                    let s = self.ctrl.status.lock().map(|s| s.clone()).unwrap_or_default();
                    ui.label(RichText::new("● OFFLINE").color(w::MUTE).monospace()).on_hover_text(if s.is_empty() {
                        "本体を掴んでいない (プラグインが非アクティブ)".into()
                    } else {
                        s
                    });
                }
            });
            block(ui, 170.0, &mut |ui| {
                w::silk(ui, "OUTPUT");
                w::meter(ui, self.peak, 160.0);
            });
            block(ui, 70.0, &mut |ui| {
                ui.add_space(12.0);
                if ui
                    .add(egui::Button::new("PANIC").min_size(vec2(64.0, 22.0)))
                    .on_hover_text("全パートに All Sound Off / Reset All Controllers を送る")
                    .clicked()
                {
                    let mut v = Vec::new();
                    for cable in 0..2u8 {
                        for ch in 0..16u8 {
                            v.push(Out::msg(cable, &[0xB0 | ch, 120, 0]));
                            v.push(Out::msg(cable, &[0xB0 | ch, 121, 0]));
                        }
                    }
                    self.ctrl.post(v);
                }
            });
        });
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(mode) = self.confirm else { return };
        let mut open = true;
        egui::Window::new("Sound generator mode")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("{} モードに切り替えます。", mode.name()));
                ui.label("音源は初期化され、すべてのパートの設定が初期値に戻ります。");
                ui.label(RichText::new("(Owner's Manual p.20 \"Switching the sound generator mode\")").color(w::SILK_DIM).size(11.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("  ENTER  ").strong()).clicked() {
                        self.switch_to = Some(mode);
                        self.confirm = None;
                    }
                    if ui.button("  EXIT  ").clicked() {
                        self.confirm = None;
                    }
                });
            });
        if !open {
            self.confirm = None;
        }
    }

    // ---- パート一覧 -------------------------------------------------------

    fn part_list(&mut self, ui: &mut Ui) {
        w::silk(ui, "MIDI INSTRUMENTS  —  PARTS");
        let editable = self.m.mode.editable();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for i in 0..PARTS {
                if i % 16 == 0 {
                    ui.add_space(if i == 0 { 2.0 } else { 6.0 });
                    ui.label(
                        RichText::new(if i == 0 { "PORT A  (bus \"PART A\")" } else { "PORT B  (bus \"PART B\")" })
                            .color(w::SILK_DIM)
                            .size(10.5),
                    );
                }
                self.part_row(ui, i, editable);
            }
        });
    }

    fn part_row(&mut self, ui: &mut Ui, i: usize, editable: bool) {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), egui::Sense::click());
        let selected = self.sel == i;
        let solo = self.m.common.solo;
        let p = &self.m.parts[i];
        // ソロ中は他のパートが鳴らない (p.27)。ソロはミュートに優先する (p.26)。
        let audible = if solo != 0 { solo as usize == i + 1 } else { !p.mute };
        let painter = ui.painter_at(rect);
        let bg = if selected {
            Color32::from_rgb(0x2c, 0x5a, 0x86)
        } else if resp.hovered() {
            w::PANEL_HI
        } else if i % 2 == 0 {
            Color32::from_rgb(0x2a, 0x2c, 0x31)
        } else {
            w::PANEL
        };
        painter.rect_filled(rect, 2.0, bg);
        let text_col = if audible { w::SILK } else { w::SILK_DIM };
        let mono = egui::FontId::monospace(12.0);
        let mut x = rect.left() + 6.0;
        let cy = rect.center().y;
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, Part::label(i), mono.clone(), if selected { Color32::WHITE } else { w::ACCENT });
        x += 36.0;

        // M / S
        let mut mute = p.mute;
        let mut is_solo = solo as usize == i + 1;
        let mrect = egui::Rect::from_center_size(egui::pos2(x + 9.0, cy), vec2(18.0, 18.0));
        let srect = egui::Rect::from_center_size(egui::pos2(x + 31.0, cy), vec2(18.0, 18.0));
        x += 46.0;
        let (ty, set, num) = if editable {
            (
                if p.drum { "DRUM" } else { "INST" },
                p.set.short(),
                if p.drum { format!("{:03}  ---", p.pc) } else { format!("{:03}  {:03}", p.pc, p.lsb) },
            )
        } else {
            ("", "", format!("{:03}  {:03}", p.pc, p.lsb))
        };
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, ty, egui::FontId::proportional(10.5), w::SILK_DIM);
        x += 36.0;
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, set, egui::FontId::proportional(10.5), w::SILK_DIM);
        x += 54.0;
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, num, mono.clone(), text_col);
        x += 70.0;
        let name = p.tone_name(self.m.mode);
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(12.5), if audible { Color32::WHITE } else { w::SILK_DIM });
        let level = p.level;

        if editable {
            if ui.put(mrect, |ui: &mut Ui| w::toggle_box(ui, &mut mute, "M", w::MUTE)).on_hover_text("ミュート (p.26)").changed() {
                self.m.parts[i].mute = mute;
            }
            if ui.put(srect, |ui: &mut Ui| w::toggle_box(ui, &mut is_solo, "S", w::SOLO)).on_hover_text("ソロ (p.27)。1 パートだけ").changed() {
                self.m.common.solo = if is_solo { i as u8 + 1 } else { 0 };
            }
        }
        let bar = egui::Rect::from_min_size(egui::pos2(rect.right() - 50.0, cy - 4.0), vec2(44.0, 8.0));
        ui.put(bar, |ui: &mut Ui| {
            w::level_bar(ui, level, audible);
            ui.allocate_response(vec2(0.0, 0.0), egui::Sense::hover())
        });

        if resp.clicked() {
            self.sel = i;
            self.tab = Tab::Part;
        }
    }

    // ---- タブ -------------------------------------------------------------

    fn tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (t, name, tip) in [
                (Tab::Part, "INST/DRUM SET", "パートの音色とパートパラメータ (p.21-37)"),
                (Tab::Effects, "EFFECTS", "音源のエフェクト (p.38-47)"),
                (Tab::All, "ALL", "全パート共通のパラメータ (p.31, p.37)"),
                (Tab::System, "SYSTEM", "Device ID や一括送信"),
            ] {
                let on = self.tab == t;
                let b = egui::Button::new(RichText::new(name).color(if on { Color32::BLACK } else { w::SILK }).strong())
                    .fill(if on { w::SILK } else { Color32::from_rgb(0x1c, 0x1d, 0x20) })
                    .min_size(vec2(110.0, 24.0));
                if ui.add(b).on_hover_text(tip).clicked() {
                    self.tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = self.show_keyboard;
                let b = egui::Button::new(RichText::new("KEYBOARD").color(if on { Color32::BLACK } else { w::SILK }).strong())
                    .fill(if on { w::ACCENT } else { Color32::from_rgb(0x1c, 0x1d, 0x20) })
                    .min_size(vec2(96.0, 24.0));
                if ui.add(b).on_hover_text("画面の鍵盤を出す / しまう。選んでいるパートを鳴らす").clicked() {
                    if on {
                        let mut v = Vec::new();
                        self.keyboard.release_all(&mut v);
                        self.ctrl.post(v);
                    }
                    self.show_keyboard = !on;
                }
            });
        });
    }

    // ---- INST/DRUM SET ----------------------------------------------------

    fn part_page(&mut self, ui: &mut Ui) {
        let i = self.sel;
        let mode = self.m.mode;
        let native = mode == Mode::Native;

        // LCD。本体の PART PARAMETER 画面 (p.21) と同じ並び。
        w::lcd(ui, |ui| {
            w::inverted(ui, "PART PARAMETER");
            let p = &self.m.parts[i];
            ui.horizontal(|ui| {
                ui.label(RichText::new(Part::label(i)).monospace().size(22.0).strong());
                ui.add_space(10.0);
                ui.label(RichText::new(p.tone_name(mode)).monospace().size(22.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(mode.name()).monospace().color(w::LCD_DIM));
                });
            });
            ui.horizontal(|ui| {
                let set = if mode.editable() { p.set.short() } else { "" };
                let txt = if mode.editable() && p.drum {
                    format!("DRUM  {:03}   ---   {set}", p.pc)
                } else if mode.editable() {
                    format!("INST  {:03}   {:03}   {set}", p.pc, p.lsb)
                } else {
                    format!("MSB {:03}  LSB {:03}  PRG {:03}", p.raw_msb, p.lsb, p.pc)
                };
                ui.label(RichText::new(txt).monospace().size(15.0));
            });
        });
        ui.add_space(6.0);

        // 音色選択。[PART◀] [PART▶] と [PREVIEW] はパネルのボタンに相当。
        ui.horizontal(|ui| {
            if ui.button("◀ PART").on_hover_text("[PART ◀] (p.21)").clicked() {
                self.sel = (self.sel + PARTS - 1) % PARTS;
            }
            if ui.button("PART ▶").on_hover_text("[PART ▶] (p.21)").clicked() {
                self.sel = (self.sel + 1) % PARTS;
            }
            ui.separator();
            if mode.editable() {
                self.tone_selector(ui, i);
            } else {
                self.raw_tone_selector(ui, i);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("PREVIEW").strong()).min_size(vec2(76.0, 22.0)))
                    .on_hover_text("選んでいる音色を試聴する (p.15)")
                    .clicked()
                {
                    self.preview(i);
                }
            });
        });
        if mode.editable() {
            ui.horizontal(|ui| self.tone_pick(ui, i));
        }
        ui.add_space(8.0);

        if !mode.editable() {
            self.gs_part_params(ui, i);
            return;
        }

        let p = &mut self.m.parts[i];
        let patch = |v: u8, patch_at: u8, f: &dyn Fn(u8) -> String| if v == patch_at { "PATCH".to_string() } else { f(v) };

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                group(ui, "LEVEL / SEND", |ui| {
                    knob(ui, "Volume", &mut p.level, 0, 127, 100, &w::plain).on_hover_text("Volume [cc#7] (p.32)");
                    knob(ui, "Pan", &mut p.pan, 0, 127, 64, &w::pan).on_hover_text("Pan [cc#10] L64-0-63R (p.32)");
                    knob(ui, "Modulation", &mut p.modulation, 0, 127, 0, &w::plain).on_hover_text("Modulation Depth [cc#1] (p.32)");
                    knob(ui, "Reverb", &mut p.reverb, 0, 127, 40, &w::plain).on_hover_text("Reverb Send Level [cc#91] (p.35)");
                    knob(ui, "Chorus", &mut p.chorus, 0, 127, 0, &w::plain).on_hover_text("Chorus Send Level [cc#93] (p.35)");
                });
                group(ui, "FILTER / ENVELOPE", |ui| {
                    knob(ui, "Resonance", &mut p.resonance, 0, 127, 64, &w::signed64).on_hover_text("TVF Resonance [cc#71] (p.33)");
                    knob(ui, "Cutoff", &mut p.cutoff, 0, 127, 64, &w::signed64).on_hover_text("TVF Cutoff Freq [cc#74] (p.33)");
                    knob(ui, "Attack", &mut p.attack, 0, 127, 64, &w::signed64).on_hover_text("TVF&TVA Attack [cc#73] (p.34)");
                    knob(ui, "Decay", &mut p.decay, 0, 127, 64, &w::signed64).on_hover_text("TVF&TVA Decay [cc#75] (p.34)");
                    knob(ui, "Release", &mut p.release, 0, 127, 64, &w::signed64).on_hover_text("TVF&TVA Release [cc#72] (p.34)");
                });
                group(ui, "VIBRATO", |ui| {
                    knob(ui, "Rate", &mut p.vib_rate, 0, 127, 64, &w::signed64).on_hover_text("Vibrato Rate [cc#76] (p.35)");
                    knob(ui, "Depth", &mut p.vib_depth, 0, 127, 64, &w::signed64).on_hover_text("Vibrato Depth [cc#77] (p.35)");
                    knob(ui, "Delay", &mut p.vib_delay, 0, 127, 64, &w::signed64).on_hover_text("Vibrato Delay [cc#78] (p.35)");
                });
            });
            ui.vertical(|ui| {
                group(ui, "PITCH", |ui| {
                    let pb_max = if native { 25 } else { 24 };
                    knob(ui, "PB Sens", &mut p.pb_range, 0, pb_max, if native { 25 } else { 2 }, &|v| patch(v, 25, &w::plain))
                        .on_hover_text("Pitch Bend Sensitivity 0-24 (PATCH は Native のみ) (p.35)");
                    knob(ui, "Fine", &mut p.fine, 14, 114, 64, &w::signed64).on_hover_text("Fine Tune -50 - +50 (p.35)");
                    knob(ui, "Coarse", &mut p.coarse, 16, 112, 64, &w::signed64).on_hover_text("Coarse Tune -48 - +48 (p.35)");
                });
                group(ui, "MONO/POLY  ·  PORTAMENTO", |ui| {
                    ui.vertical(|ui| {
                        let mp: &[(u8, &str)] = if native { &[(1, "POLY"), (0, "MONO"), (2, "PATCH")] } else { &[(1, "POLY"), (0, "MONO")] };
                        combo(ui, "monopoly", "Mono/Poly", &mut p.mono_poly, mp, 80.0)
                            .on_hover_text("Mono/Poly [cc#126/127] (p.35)");
                        let sw: &[(u8, &str)] = if native { &[(0, "OFF"), (1, "ON"), (2, "PATCH")] } else { &[(0, "OFF"), (1, "ON")] };
                        combo(ui, "portasw", "Portamento", &mut p.porta_sw, sw, 80.0)
                            .on_hover_text("Portamento SW [cc#65] (p.33)");
                    });
                    let pt_max = if native { 128 } else { 127 };
                    knob(ui, "Porta Time", &mut p.porta_time, 0, pt_max, if native { 128 } else { 0 }, &|v| patch(v, 128, &w::plain))
                        .on_hover_text("Portamento Time [cc#5] (p.33)");
                });
                if native {
                    group(ui, "RECEIVE  ·  KEYBOARD  (Native)", |ui| {
                        ui.vertical(|ui| {
                            let mut rx = if p.rx_sw { p.rx_ch } else { 16 };
                            let opts: Vec<(u8, String)> = (0..16u8).map(|c| (c, format!("{}", c + 1))).chain([(16, "OFF".to_string())]).collect();
                            let opts: Vec<(u8, &str)> = opts.iter().map(|(v, s)| (*v, s.as_str())).collect();
                            if combo(ui, "rxch", "Rx CH", &mut rx, &opts, 60.0)
                                .on_hover_text("受信チャンネル。OFF でエクスクルーシブ以外を無視 (p.36)")
                                .changed()
                            {
                                p.rx_sw = rx < 16;
                                if rx < 16 {
                                    p.rx_ch = rx;
                                }
                            }
                            combo(ui, "legato", "Legato", &mut p.legato, &[(0, "OFF"), (1, "ON"), (2, "PATCH")], 60.0)
                                .on_hover_text("Legato SW [cc#68] (p.36)");
                        });
                        knob(ui, "Vel Sens", &mut p.vel_sens, 1, 127, 64, &w::signed64).on_hover_text("Part Velocity Sens Offset -63 - +63 (p.36)");
                    });
                    group(ui, "KEY RANGE  (Native)", |ui| {
                        let up = p.key_upper;
                        knob(ui, "Lower", &mut p.key_lower, 0, up, 0, &w::note).on_hover_text("Part Keyboard Range Lower (p.36)");
                        let lo = p.key_lower;
                        knob(ui, "Upper", &mut p.key_upper, lo, 127, 127, &w::note).on_hover_text("Part Keyboard Range Upper (p.36)");
                        knob(ui, "Fade L", &mut p.fade_lower, 0, 127, 0, &w::plain).on_hover_text("Part Keyboard Fade Width Lower (p.36)");
                        knob(ui, "Fade U", &mut p.fade_upper, 0, 127, 0, &w::plain).on_hover_text("Part Keyboard Fade Width Upper (p.36)");
                    });
                    group(ui, "OUTPUT  (Native)", |ui| {
                        ui.vertical(|ui| {
                            combo(ui, "outassign", "Output", &mut p.out_assign, &[(OUT_MFX, "MFX"), (OUT_A, "A"), (OUT_PATCH, "PATCH")], 70.0)
                                .on_hover_text("PART OUTPUT ASSIGN (p.36)");
                            combo(ui, "outmfx", "MFX", &mut p.out_mfx, &[(0, "MFX-A"), (1, "MFX-B"), (2, "MFX-C")], 70.0)
                                .on_hover_text("PART OUTPUT MFX SELECT (p.37)");
                        });
                        knob(ui, "Dry Send", &mut p.dry, 0, 127, 127, &w::plain).on_hover_text("PART DRY SEND LEVEL (p.37)");
                    });
                }
            });
        });

        group(ui, "SCALE / OCTAVE TUNING ADJUST", |ui| {
            const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            ui.vertical(|ui| {
                for row in [0..6, 6..12] {
                    ui.horizontal(|ui| {
                        for k in row {
                            knob(ui, NAMES[k], &mut p.scale[k], 0, 127, 64, &w::signed64).on_hover_text("Scale Tune -64 - +63 (p.36)");
                        }
                    });
                }
            });
        });
    }

    /// INST/DRUM とセット。
    fn tone_selector(&mut self, ui: &mut Ui, i: usize) {
        let mode = self.m.mode;
        let native = mode == Mode::Native;
        let p = &mut self.m.parts[i];

        let mut drum = p.drum;
        let mut changed = false;
        ui.selectable_value(&mut drum, false, "INST");
        ui.selectable_value(&mut drum, true, "DRUM").on_hover_text("Part Mode (p.22)");
        if drum != p.drum {
            p.drum = drum;
            if drum && p.set.is_special() {
                p.set = SoundSet::Contemporary;
            }
            p.lsb = 0;
            p.pc = if drum { 1 } else { p.pc };
            changed = true;
        }

        // セット。Special は Native の Inst パートだけ (p.23)。
        let sets: Vec<SoundSet> = SoundSet::ALL
            .into_iter()
            .filter(|s| !s.is_special() || (native && !p.drum))
            .collect();
        egui::ComboBox::from_id_salt("set")
            .width(96.0)
            .selected_text(p.set.name())
            .show_ui(ui, |ui| {
                for s in sets {
                    if ui.selectable_label(p.set == s, s.name()).clicked() && p.set != s {
                        p.set = s;
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text("Sound set (p.23)");

        if changed {
            // セットを変えて今の番号がそのセットに無ければ capital に戻す。
            if !p.drum && tones::inst_name(p.set, p.pc, p.lsb).is_none() {
                p.lsb = 0;
                if tones::inst_name(p.set, p.pc, 0).is_none() {
                    p.pc = 1;
                }
            }
            if p.drum && tones::drum_name(p.set, p.pc).is_none() {
                p.pc = 1;
            }
        }
    }

    /// 楽器 (capital sound) と Variation、またはドラムセット。
    fn tone_pick(&mut self, ui: &mut Ui, i: usize) {
        let p = &mut self.m.parts[i];
        let tag = |ui: &mut Ui, t: &str| ui.label(RichText::new(t).color(w::SILK_DIM).size(11.0));
        if p.drum {
            tag(ui, "DRUM SET");
            let cur = tones::drum_name(p.set, p.pc).map(|n| format!("{:03} {n}", p.pc)).unwrap_or(format!("{:03}", p.pc));
            egui::ComboBox::from_id_salt("drumset")
                .width(230.0)
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (pc, names) in tones::DRUMS {
                        let n = names[p.set.gm2_index().unwrap_or(1)];
                        if ui.selectable_label(p.pc == *pc, format!("{pc:03} {n}")).clicked() {
                            p.pc = *pc;
                        }
                    }
                })
                .response
                .on_hover_text("Drum set (p.25, p.139)");
        } else {
            tag(ui, "INST");
            let cur = tones::inst_name(p.set, p.pc, 0).unwrap_or("-");
            egui::ComboBox::from_id_salt("inst")
                .width(230.0)
                .height(420.0)
                .selected_text(format!("{:03} {cur}", p.pc))
                .show_ui(ui, |ui| {
                    for pc in 1..=128u8 {
                        let n = tones::inst_name(p.set, pc, 0).unwrap_or("-");
                        if ui.selectable_label(p.pc == pc, format!("{pc:03} {n}")).clicked() && p.pc != pc {
                            p.pc = pc;
                            p.lsb = 0;
                        }
                    }
                })
                .response
                .on_hover_text("Instrument (capital sound, p.24)");
            tag(ui, "VARIAT");
            let vars: Vec<(u8, &str)> = tones::variations(p.set, p.pc).collect();
            let cur = vars.iter().find(|(l, _)| *l == p.lsb).map(|(l, n)| format!("{l:03} {n}")).unwrap_or(format!("{:03}", p.lsb));
            egui::ComboBox::from_id_salt("var")
                .width(200.0)
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (l, n) in &vars {
                        if ui.selectable_label(p.lsb == *l, format!("{l:03} {n}")).clicked() {
                            p.lsb = *l;
                        }
                    }
                })
                .response
                .on_hover_text("Variation (p.24)");
        }
    }

    /// GS / XGlite。名前の表は持たないので番号で選ぶ。
    fn raw_tone_selector(&mut self, ui: &mut Ui, i: usize) {
        let p = &mut self.m.parts[i];
        ui.label("Bank MSB");
        ui.add(egui::DragValue::new(&mut p.raw_msb).range(0..=127));
        ui.label("LSB");
        ui.add(egui::DragValue::new(&mut p.lsb).range(0..=127));
        ui.label("Program");
        ui.add(egui::DragValue::new(&mut p.pc).range(1..=128));
    }

    fn gs_part_params(&mut self, ui: &mut Ui, i: usize) {
        let p = &mut self.m.parts[i];
        group(ui, "CONTROL CHANGE", |ui| {
            knob(ui, "Volume", &mut p.level, 0, 127, 100, &w::plain).on_hover_text("cc#7");
            knob(ui, "Pan", &mut p.pan, 0, 127, 64, &w::pan).on_hover_text("cc#10");
            knob(ui, "Reverb", &mut p.reverb, 0, 127, 40, &w::plain).on_hover_text("cc#91");
            knob(ui, "Chorus", &mut p.chorus, 0, 127, 0, &w::plain).on_hover_text("cc#93");
            knob(ui, "Modulation", &mut p.modulation, 0, 127, 0, &w::plain).on_hover_text("cc#1");
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "GS / XGlite モードの音源パラメータは本体パネルからは編集できない (Owner's Manual p.17)。\n\
                 ここではパートのチャンネルに Bank Select / Program Change と上の CC だけを送る。",
            )
            .color(w::SILK_DIM),
        );
    }

    fn preview(&mut self, i: usize) {
        let p = &self.m.parts[i];
        if !p.rx_sw {
            return;
        }
        if let Some((_, off)) = self.preview_off.take() {
            self.ctrl.post([off]);
        }
        let ch = p.rx_ch & 0x0F;
        let note = if p.drum { 38 } else { 60 };
        let cable = Part::cable(i);
        self.ctrl.post([Out::msg(cable, &[0x90 | ch, note, 100])]);
        self.preview_off = Some((Instant::now() + Duration::from_millis(700), Out::msg(cable, &[0x80 | ch, note, 0])));
    }

    // ---- EFFECTS ----------------------------------------------------------

    fn effects_page(&mut self, ui: &mut Ui) {
        let mode = self.m.mode;
        if !mode.editable() {
            ui.label(RichText::new("エフェクトを編集できるのは GM2 / Native モードだけ (Owner's Manual p.39)。").color(w::SILK_DIM));
            return;
        }
        let c = &mut self.m.common;
        let sources: Vec<(u8, String)> = std::iter::once((0u8, "COMMON".to_string()))
            .chain((1..=32u8).map(|n| (n, format!("PART {}", Part::label(n as usize - 1)))))
            .collect();
        let sources: Vec<(u8, &str)> = sources.iter().map(|(v, s)| (*v, s.as_str())).collect();
        let listed = |names: &[&'static str]| -> Vec<(u8, &'static str)> {
            names.iter().enumerate().map(|(k, n)| (k as u8, *n)).collect()
        };

        if mode == Mode::Gm2 {
            group(ui, "REVERB  (System Effect)", |ui| {
                ui.vertical(|ui| {
                    let old = c.gm2_reverb_type;
                    combo(ui, "gm2rev", "Type", &mut c.gm2_reverb_type, &sd90::GM2_REVERB_TYPES, 120.0)
                        .on_hover_text("Reverb Type (p.40)");
                    if c.gm2_reverb_type != old {
                        c.gm2_reverb_time = sd90::gm2_reverb_time_default(c.gm2_reverb_type);
                    }
                });
                let d = sd90::gm2_reverb_time_default(c.gm2_reverb_type);
                knob(ui, "Time", &mut c.gm2_reverb_time, 0, 127, d, &w::plain).on_hover_text("Reverb Time (p.40)");
                knob(ui, "Level", &mut c.reverb_level, 0, 127, 100, &w::plain).on_hover_text("Reverb Level");
            });
            group(ui, "CHORUS  (System Effect)", |ui| {
                ui.vertical(|ui| {
                    let old = c.gm2_chorus_type;
                    combo(ui, "gm2cho", "Type", &mut c.gm2_chorus_type, &listed(&sd90::GM2_CHORUS_TYPES), 120.0)
                        .on_hover_text("Chorus Type (p.40)");
                    if c.gm2_chorus_type != old {
                        c.gm2_chorus = sd90::gm2_chorus_default(c.gm2_chorus_type);
                    }
                });
                let d = sd90::gm2_chorus_default(c.gm2_chorus_type);
                let [rate, depth, fb, send] = &mut c.gm2_chorus;
                knob(ui, "Mod Rate", rate, 0, 127, d[0], &w::plain).on_hover_text("Mod Rate (p.41)");
                knob(ui, "Mod Depth", depth, 0, 127, d[1], &w::plain).on_hover_text("Mod Depth (p.41)");
                knob(ui, "Feedback", fb, 0, 127, d[2], &w::plain).on_hover_text("Feedback (p.41)");
                knob(ui, "Send Rev", send, 0, 127, d[3], &w::plain).on_hover_text("Send To Reverb (p.41)");
                knob(ui, "Level", &mut c.chorus_level, 0, 127, 127, &w::plain).on_hover_text("Chorus Level");
            });
        } else {
            group(ui, "REVERB  (System Effect)", |ui| {
                ui.vertical(|ui| {
                    combo(ui, "rev", "Type", &mut c.reverb_type, &listed(&sd90::REVERB_TYPES), 120.0)
                        .on_hover_text("Reverb Type (p.42)");
                    combo(ui, "revsrc", "Source", &mut c.reverb_source, &sources, 120.0)
                        .on_hover_text("Reverb Source: COMMON か、パートの音色の設定を使う (p.42)");
                });
                knob(ui, "Level", &mut c.reverb_level, 0, 127, 100, &w::plain).on_hover_text("Reverb Level (p.42)");
            });
            group(ui, "CHORUS  (System Effect)", |ui| {
                ui.vertical(|ui| {
                    combo(ui, "cho", "Type", &mut c.chorus_type, &listed(&sd90::CHORUS_TYPES), 120.0)
                        .on_hover_text("Chorus Type (p.44)");
                    combo(ui, "choout", "Output", &mut c.chorus_out, &listed(&sd90::CHORUS_OUTS), 120.0)
                        .on_hover_text("Chorus Output Select (p.44)");
                    combo(ui, "chosrc", "Source", &mut c.chorus_source, &sources, 120.0).on_hover_text("Chorus Source (p.44)");
                });
                knob(ui, "Level", &mut c.chorus_level, 0, 127, 127, &w::plain).on_hover_text("Chorus Level (p.44)");
            });
            let types = listed(&tones::MFX_TYPES);
            for n in 0..3 {
                let title = ["MFX-A  (Multi-effect)", "MFX-B  (Multi-effect)", "MFX-C  (Multi-effect)"][n];
                group(ui, title, |ui| {
                    ui.vertical(|ui| {
                        let old = c.mfx[n][0];
                        combo(ui, &format!("mfxt{n}"), "Type", &mut c.mfx[n][0], &types, 190.0)
                            .on_hover_text("MFX Type (p.45)。パラメータは本体のタイプ既定値のまま");
                        if c.mfx[n][0] != old {
                            // COMMON 以外だとパートの音色の MFX が使われ、ここの設定は効かない。
                            c.mfx_source[n] = 0;
                        }
                        combo(ui, &format!("mfxs{n}"), "Source", &mut c.mfx_source[n], &sources, 190.0)
                            .on_hover_text("MFX Source: COMMON か、パートの音色 (Enhanced 等) の MFX を使う");
                    });
                    let [_, dry, cho, rev] = &mut c.mfx[n];
                    knob(ui, "Dry", dry, 0, 127, 127, &w::plain).on_hover_text("MFX Dry Send Level");
                    knob(ui, "Chorus", cho, 0, 127, 0, &w::plain).on_hover_text("MFX Chorus Send Level");
                    knob(ui, "Reverb", rev, 0, 127, 0, &w::plain).on_hover_text("MFX Reverb Send Level");
                });
            }
        }
        eq_group(ui, c, &mut self.eq_link);
        if mode == Mode::Gm2 {
            ui.label(RichText::new("MFX は GM2 モードでは選べない (p.39)。").color(w::SILK_DIM).size(11.0));
        } else {
            ui.label(
                RichText::new("パートの音を MFX に通すには INST/DRUM SET の OUTPUT で Output = MFX と MFX-A/B/C を選ぶ (p.36-37)。")
                    .color(w::SILK_DIM)
                    .size(11.0),
            );
        }
    }

    // ---- ALL --------------------------------------------------------------

    fn all_page(&mut self, ui: &mut Ui) {
        if !self.m.mode.editable() {
            ui.label(RichText::new("GS / XGlite モードでは編集できない (p.28)。").color(w::SILK_DIM));
            return;
        }
        w::lcd(ui, |ui| {
            w::inverted(ui, "ALL");
            let c = &self.m.common;
            let hz = 440.0 * 2f32.powf((c.master_tune as f32 - 1024.0) / 10.0 / 1200.0);
            ui.label(RichText::new(format!("Master Volume  {:>6}", c.master_level)).monospace().size(15.0));
            ui.label(RichText::new(format!("Master Fine    {hz:>6.1}")).monospace().size(15.0));
            ui.label(RichText::new(format!("Master Coarse  {:>6}", c.key_shift as i32 - 64)).monospace().size(15.0));
        });
        ui.add_space(8.0);
        let c = &mut self.m.common;
        group(ui, "PARAMETERS COMMON TO ALL PARTS", |ui| {
            knob(ui, "Vol", &mut c.master_level, 0, 127, 127, &w::plain).on_hover_text("Master Volume 0-127 (p.37)");
            ui.vertical(|ui| {
                ui.label(RichText::new("Fine (Master Fine Tuning)").color(w::SILK_DIM).size(11.0));
                let mut t = c.master_tune as i32;
                let r = ui.add(
                    egui::Slider::new(&mut t, 24..=2024)
                        .show_value(true)
                        .custom_formatter(|v, _| format!("{:.1} Hz", 440.0 * 2f64.powf((v - 1024.0) / 10.0 / 1200.0)))
                        .custom_parser(|s| {
                            let hz: f64 = s.trim().trim_end_matches("Hz").trim().parse().ok()?;
                            Some(1024.0 + 12000.0 * (hz / 440.0).log2())
                        }),
                );
                if r.on_hover_text("415.3 - 440.0 - 466.2 Hz。A4 の周波数 (p.37)").changed() {
                    c.master_tune = t.clamp(24, 2024) as u16;
                }
                if ui.small_button("440.0 Hz").clicked() {
                    c.master_tune = 1024;
                }
            });
            knob(ui, "Coarse", &mut c.key_shift, 40, 88, 64, &w::signed64).on_hover_text("Master Coarse Tuning -24 - +24。ドラムパートには効かない (p.37)");
        });
    }

    // ---- SYSTEM -----------------------------------------------------------

    fn system_page(&mut self, ui: &mut Ui) {
        group(ui, "MIDI", |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Device ID");
                    let mut id = self.m.device_id as i32 + 1;
                    if ui
                        .add(egui::DragValue::new(&mut id).range(17..=32))
                        .on_hover_text("本体の Device ID Number に合わせる。既定 17 (p.72)")
                        .changed()
                    {
                        self.m.device_id = (id - 1) as u8;
                    }
                });
                ui.checkbox(&mut self.m.recall, "プロジェクトを開いたときにこの設定を本体へ送る")
                    .on_hover_text("エディタで何か変更すると自動で入る。切ると DAW 側のデータ (曲頭の GM リセット等) に任せる");
                ui.horizontal(|ui| {
                    if ui.button("SEND ALL").on_hover_text("モード切り替えから始めて、今の設定一式を本体に送る").clicked() {
                        self.commit_model();
                        self.ctrl.send_all();
                    }
                    if ui
                        .button("INIT")
                        .on_hover_text("今のモードの初期化メッセージを送り、エディタの設定も初期値に戻す (p.70)")
                        .clicked()
                    {
                        self.confirm = Some(self.m.mode);
                    }
                });
            });
        });
        group(ui, "STATUS", |ui| {
            ui.vertical(|ui| {
                let online = self.ctrl.online.load(Relaxed);
                let status = self.ctrl.status.lock().map(|s| s.clone()).unwrap_or_default();
                ui.label(if online { RichText::new("接続中").color(w::OK) } else { RichText::new("未接続").color(w::MUTE) });
                if !status.is_empty() {
                    ui.label(RichText::new(status).color(w::SILK_DIM));
                }
                let lat = self.ctrl.latency.load(Relaxed);
                let set_hz = self.ctrl.host_rate.load(Relaxed).max(1) as f64;
                ui.label(format!("申告レイテンシ: {lat} フレーム ({:.2} ms)", lat as f64 * 1000.0 / set_hz));
                if let Some(e) = self.ctrl.engine.lock().ok().and_then(|e| e.upgrade()) {
                    let (frames, over, dropped) = e.diag();
                    ui.label(format!("取り込み: {frames} フレーム ({:.0} 秒)", frames as f64 / 48000.0));
                    // 2 秒ごとに、ホストと SD-90 がそれぞれ実際に何 Hz で回っているかを測る。
                    let host = self.ctrl.host_frames.load(Relaxed);
                    let now = Instant::now();
                    match self.rate_snap {
                        Some((t0, h0, c0)) if now.duration_since(t0) >= Duration::from_secs(2) => {
                            let dt = now.duration_since(t0).as_secs_f64();
                            let (hz_h, hz_d) = ((host - h0) as f64 / dt, (frames - c0) as f64 / dt);
                            // 水晶のずれは高々 0.01% 程度。リサンプラが吸える 0.2% を超えるなら、
                            // ホストが伝えてきたレートと違うレートで回っている
                            // (Cubase はプロジェクトとインターフェイスのレートが違っても再生を続ける)。
                            let off = (hz_h / set_hz) / (hz_d / 48_000.0) - 1.0;
                            self.rate_warn = hz_d > 0.0 && off.abs() > 0.002;
                            self.rate_text = if hz_d > 0.0 {
                                format!(
                                    "ホスト: {hz_h:.0} Hz (設定 {set_hz:.0} Hz)   SD-90: {hz_d:.0} Hz   差: {:+.3} %",
                                    off * 100.0
                                )
                            } else {
                                String::new()
                            };
                            self.rate_snap = Some((now, host, frames));
                        }
                        None => self.rate_snap = Some((now, host, frames)),
                        _ => {}
                    }
                    if !self.rate_text.is_empty() {
                        ui.label(&self.rate_text);
                    }
                    if self.rate_warn {
                        ui.label(
                            RichText::new(
                                "ホストの実際のレートがプロジェクトの設定と違う。オーディオインターフェイスの
                                 サンプリングレートをプロジェクトと揃えること (ずれた分はノイズになる)",
                            )
                            .color(w::MUTE),
                        );
                    }
                    ui.label(format!(
                        "読み出し比: {:.5}   水位: {} フレーム",
                        f32::from_bits(self.ctrl.reader_ratio.load(Relaxed)),
                        self.ctrl.reader_level.load(Relaxed),
                    ));
                    ui.label(format!(
                        "リング溢れ: {over}   アンダーラン: {}   溜まりすぎ: {}   送出キュー溢れ: {dropped}",
                        self.ctrl.reader_underruns.load(Relaxed),
                        self.ctrl.reader_overfulls.load(Relaxed),
                    ));
                }
                ui.label(format!("エディタからの送信待ち: {}", self.ctrl.pending()));
            });
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "パラメータの意味と範囲は EDIROL SD-90 Owner's Manual / MIDI Implementation に従う。\n\
                 編集内容は本体の内部メモリには保存されない (p.28)。プロジェクトに保存され、開いたときに送り直す。",
            )
            .color(w::SILK_DIM)
            .size(11.0),
        );
    }
}

/// 枠付きのまとまり。中身は横に並ぶ。
fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .fill(Color32::from_rgb(0x2e, 0x31, 0x36))
        .stroke(egui::Stroke::new(1.0_f32, w::PANEL_LINE))
        .rounding(4.0)
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new(title).color(w::SILK_DIM).size(10.5).strong());
            ui.horizontal(|ui| add(ui));
        });
    ui.add_space(4.0);
}

/// ラベル付きのコンボボックス。値は生の値。
fn combo(ui: &mut Ui, id: &str, label: &str, value: &mut u8, opts: &[(u8, &str)], width: f32) -> egui::Response {
    let cur = opts.iter().find(|(v, _)| v == value).map(|(_, s)| *s).unwrap_or("?");
    let mut changed = false;
    let mut r = ui
        .horizontal(|ui| {
            ui.add_sized(vec2(64.0, 18.0), egui::Label::new(RichText::new(label).color(w::SILK_DIM).size(11.0)));
            egui::ComboBox::from_id_salt(id)
                .width(width)
                .height(360.0)
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (v, s) in opts {
                        if ui.selectable_label(*value == *v, *s).clicked() && *value != *v {
                            *value = *v;
                            changed = true;
                        }
                    }
                })
                .response
        })
        .inner;
    if changed {
        r.mark_changed();
    }
    r
}

fn eq_group(ui: &mut Ui, c: &mut sd90::Common, link: &mut bool) {
    group(ui, "EQ  (Equalizer)", |ui| {
        ui.vertical(|ui| {
            let mut on = c.eq_on as u8;
            combo(ui, "eqsw", "Switch", &mut on, &[(0, "BYPASS"), (1, "ON")], 80.0).on_hover_text("EQ Switch (p.41)");
            c.eq_on = on != 0;
            if ui.checkbox(link, "L/R link").on_hover_text("EQL (Lch) と EQR (Rch) を同じ値にする").changed() && *link {
                c.eq[1] = c.eq[0];
            }
        });
        let sides: &[usize] = if *link { &[0] } else { &[0, 1] };
        for &s in sides {
            ui.vertical(|ui| {
                let lr = if *link { "L+R" } else if s == 0 { "EQL" } else { "EQR" };
                let lows: Vec<(u8, &str)> = sd90::EQ_LOW_FREQS.iter().enumerate().map(|(k, n)| (k as u8, *n)).collect();
                let highs: Vec<(u8, &str)> = sd90::EQ_HIGH_FREQS.iter().enumerate().map(|(k, n)| (k as u8, *n)).collect();
                combo(ui, &format!("eqlf{s}"), &format!("{lr} Low"), &mut c.eq[s][0], &lows, 80.0).on_hover_text("Low Frequency (p.41)");
                combo(ui, &format!("eqhf{s}"), &format!("{lr} High"), &mut c.eq[s][2], &highs, 80.0).on_hover_text("High Frequency (p.41)");
            });
            let gain = |v: u8| {
                let d = v as i32 - 15;
                if d > 0 { format!("+{d}") } else { format!("{d}") }
            };
            let [_, lg, _, hg] = &mut c.eq[s];
            knob(ui, "Low Gain", lg, 0, 30, 15, &gain).on_hover_text("Low Gain -15 - +15 dB (p.41)");
            knob(ui, "High Gain", hg, 0, 30, 15, &gain).on_hover_text("High Gain -15 - +15 dB (p.41)");
        }
        if *link {
            c.eq[1] = c.eq[0];
        }
    });
}

/// システムの日本語フォント。1 回だけ読んでプロセスが終わるまで持つ。
fn japanese_font() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *FONT.get_or_init(|| {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"]
            .iter()
            .find_map(|f| std::fs::read(format!(r"{dir}\Fonts\{f}")).ok())
            .map(|v| &*Box::leak(v.into_boxed_slice()))
    })
}
