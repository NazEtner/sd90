//! エディタが編集する SD-90 の音源設定と、それを本体に送る MIDI メッセージ。
//!
//! 値はすべて MIDI Implementation のアドレスマップに書かれた生の値で持つ
//! (例: Coarse Tune は 16-112 で 64 が 0)。表示への換算はエディタ側でやる。
//! こうしておけばここは「値を並べて DT1 に包む」だけで済み、換算の誤りが
//! 本体に送る値に混ざらない。
//!
//! 出典:
//! * MIDI Implementation (SD-90_MI.pdf) "2. Parameter address map" (p.7-)
//! * Owner's Manual (SD-90_OM.pdf) "Controlling the SD-90 via MIDI" (p.77-)
//!
//! 送り分けの方針はマニュアルの記述どおり:
//! * 音源モードの切り替えは p.77 の 4 つのメッセージ。
//! * 音色の選択はパートのチャンネルに Bank Select + Program Change (p.79-83)。
//!   GM2 モードの音色セットだけは DT1 (`10 00 pp 3F`, p.79)。
//! * それ以外のパートパラメータ / エフェクト / 全パート共通は DT1。
//! * GM2 モードのリバーブ / コーラスのタイプは Universal Realtime の
//!   Global Parameter Control (MI p.3)。

use crate::tones::SoundSet;

/// 32 パート。A01-A16 が Port A (cable 0)、B01-B16 が Port B (cable 1)。
pub const PARTS: usize = 32;

/// Roland の機種 ID (SD-90 = 00H 48H)。
const MODEL_ID: [u8; 2] = [0x00, 0x48];
/// DT1 に詰める最大のデータ長。パート 1 本ぶん (07-24) が 30 バイト。
pub const MAX_MSG: usize = 48;

/// 音源モード (Owner's Manual p.17)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Gm2,
    Native,
    Gs,
    XgLite,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Gm2, Mode::Native, Mode::Gs, Mode::XgLite];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Gm2 => "GM2",
            Mode::Native => "Native",
            Mode::Gs => "GS",
            Mode::XgLite => "XGlite",
        }
    }

    /// パネルから (= このエディタから) パラメータを編集できるのは GM2 と
    /// Native だけ (p.28 "About parameters")。
    pub fn editable(self) -> bool {
        matches!(self, Mode::Gm2 | Mode::Native)
    }

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Mode {
        match v {
            1 => Mode::Native,
            2 => Mode::Gs,
            3 => Mode::XgLite,
            _ => Mode::Gm2,
        }
    }

    /// 切り替えのメッセージ (p.77) を `device_id` 宛てに。Roland 形式の 2 つは
    /// Device ID を含むので、本体の設定を変えている場合はそれに合わせる。
    pub fn reset(self, device_id: u8) -> Out {
        let mut b = self.reset_message().to_vec();
        if matches!(self, Mode::Native | Mode::Gs) {
            b[2] = device_id;
        }
        Out::msg(0, &b)
    }

    /// 切り替えのメッセージ (p.77)。送ると音源は初期化される。
    pub fn reset_message(self) -> &'static [u8] {
        match self {
            Mode::Gm2 => &[0xF0, 0x7E, 0x7F, 0x09, 0x03, 0xF7],
            Mode::Native => &[
                0xF0, 0x41, 0x10, 0x00, 0x48, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7,
            ],
            Mode::Gs => &[
                0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7,
            ],
            Mode::XgLite => &[0xF0, 0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00, 0xF7],
        }
    }
}

/// パートパラメータ。Multitimbre Part (`10 00 2p 00` から) の生の値。
#[derive(Clone, PartialEq, Debug)]
pub struct Part {
    /// INST / DRUM (Part Mode, p.22)。
    pub drum: bool,
    pub set: SoundSet,
    /// Instrument number 1-128 (= Program Change + 1)。
    pub pc: u8,
    /// Variation number (= Bank Select LSB)。
    pub lsb: u8,
    /// GS / XGlite モードで使う生の Bank Select MSB。
    pub raw_msb: u8,

    /// 00 Receive Channel 0-15。
    pub rx_ch: u8,
    /// 01 Receive Switch。
    pub rx_sw: bool,

    /// 07 Part Level (CC#7) 0-127。
    pub level: u8,
    /// 08 Part Pan (CC#10) 0-127, 64 = CENTER。
    pub pan: u8,
    /// 09 Coarse Tune 16-112 (-48 - +48)。
    pub coarse: u8,
    /// 0A Fine Tune 14-114 (-50 - +50)。
    pub fine: u8,
    /// 0B Mono/Poly 0 MONO, 1 POLY, 2 PATCH。
    pub mono_poly: u8,
    /// 0C Legato 0 OFF, 1 ON, 2 PATCH。
    pub legato: u8,
    /// 0D Pitch Bend Range 0-24, 25 = PATCH。
    pub pb_range: u8,
    /// 0E Portamento Switch 0 OFF, 1 ON, 2 PATCH。
    pub porta_sw: u8,
    /// 0F-10 Portamento Time 0-127, 128 = PATCH (2 ニブル)。
    pub porta_time: u8,
    /// 11 Cutoff (CC#74) 0-127 (-64 - +63)。
    pub cutoff: u8,
    /// 12 Resonance (CC#71)。
    pub resonance: u8,
    /// 13 Attack (CC#73)。
    pub attack: u8,
    /// 14 Release (CC#72)。
    pub release: u8,
    /// 15 Octave Shift 61-67 (-3 - +3)。
    pub octave: u8,
    /// 16 Velocity Sens Offset 1-127 (-63 - +63)。
    pub vel_sens: u8,
    /// 17 Keyboard Range Lower。
    pub key_lower: u8,
    /// 18 Keyboard Range Upper。
    pub key_upper: u8,
    /// 19 Keyboard Fade Width Lower。
    pub fade_lower: u8,
    /// 1A Keyboard Fade Width Upper。
    pub fade_upper: u8,
    /// 1B Mute Switch。
    pub mute: bool,
    /// 1C Dry Send Level。
    pub dry: u8,
    /// 1D Chorus Send Level (CC#93)。
    pub chorus: u8,
    /// 1E Reverb Send Level (CC#91)。
    pub reverb: u8,
    /// 1F Output Assign 0 MFX, 1 A, 13 PATCH。
    pub out_assign: u8,
    /// 20 Output MFX Select 0-2 (MFX-A/B/C)。
    pub out_mfx: u8,
    /// 21 Decay (CC#75)。
    pub decay: u8,
    /// 22 Vibrato Rate (CC#76)。
    pub vib_rate: u8,
    /// 23 Vibrato Depth (CC#77)。
    pub vib_depth: u8,
    /// 24 Vibrato Delay (CC#78)。
    pub vib_delay: u8,
    /// 33-3E Scale Tune C-B 0-127 (-64 - +63)。
    pub scale: [u8; 12],
    /// Modulation Depth (CC#1)。アドレスマップに無いので CC で送る。
    pub modulation: u8,
}

pub const OUT_MFX: u8 = 0;
pub const OUT_A: u8 = 1;
pub const OUT_PATCH: u8 = 13;

impl Part {
    /// 音源モード `mode` で初期化された直後の値。
    ///
    /// MIDI Implementation の default 欄と、Owner's Manual のパートパラメータ
    /// 一覧 (p.32-37, 太字が既定値) から。
    pub fn initial(mode: Mode, index: usize) -> Part {
        let ch = (index % 16) as u8;
        // A10 / B10 がドラム (p.19)。
        let drum = ch == 9;
        let native = mode == Mode::Native;
        let (set, pc) = if native {
            // Patch Bank Select MSB の default: PART 1: 80, PART 2: 81,
            // PART 3: 99, PART 10: 105, PART 26: 105, OTHER: 97。
            let set = match index {
                0 => SoundSet::Special1,
                1 => SoundSet::Special2,
                2 => SoundSet::Enhanced,
                _ => SoundSet::Contemporary,
            };
            (set, 1)
        } else {
            // GM2 Instrument Set Select の default は Contemporary。
            (SoundSet::Contemporary, 1)
        };
        Part {
            drum,
            set,
            pc,
            lsb: 0,
            raw_msb: 0,
            rx_ch: ch,
            rx_sw: true,
            level: 100,
            pan: 64,
            coarse: 64,
            fine: 64,
            // GM2 モードでは PATCH を選べない (p.35 "PATCH: Native mode only")。
            mono_poly: if native { 2 } else { 1 },
            legato: if native { 2 } else { 0 },
            pb_range: if native { 25 } else { 2 },
            porta_sw: if native { 2 } else { 0 },
            porta_time: if native { 128 } else { 0 },
            cutoff: 64,
            resonance: 64,
            attack: 64,
            release: 64,
            octave: 64,
            vel_sens: 64,
            key_lower: 0,
            key_upper: 127,
            fade_lower: 0,
            fade_upper: 0,
            mute: false,
            dry: 127,
            chorus: 0,
            reverb: 40,
            out_assign: if native && index < 3 { OUT_PATCH } else { OUT_A },
            out_mfx: if native && index < 3 { index as u8 } else { 0 },
            decay: 64,
            vib_rate: 64,
            vib_depth: 64,
            vib_delay: 64,
            scale: [64; 12],
            modulation: 0,
        }
    }

    /// Port A = cable 0、Port B = cable 1。
    pub fn cable(index: usize) -> u8 {
        (index / 16) as u8
    }

    /// LCD の表記 (A01 ... B16)。
    pub fn label(index: usize) -> String {
        format!("{}{:02}", if index < 16 { 'A' } else { 'B' }, index % 16 + 1)
    }

    /// 選ばれている音色の名前。
    pub fn tone_name(&self, mode: Mode) -> String {
        if !mode.editable() {
            return format!("Bank {}:{} Prg {}", self.raw_msb, self.lsb, self.pc);
        }
        let name = if self.drum {
            crate::tones::drum_name(self.set, self.pc)
        } else {
            crate::tones::inst_name(self.set, self.pc, self.lsb)
        };
        name.map(str::to_string)
            .unwrap_or_else(|| format!("({:03}:{:03})", self.pc, self.lsb))
    }

    /// 07-24 をまとめて 1 本の DT1 にする用の並び。
    fn block_07_24(&self) -> [u8; 0x24 - 0x07 + 1] {
        [
            self.level,
            self.pan,
            self.coarse,
            self.fine,
            self.mono_poly,
            self.legato,
            self.pb_range,
            self.porta_sw,
            self.porta_time >> 4,
            self.porta_time & 0x0F,
            self.cutoff,
            self.resonance,
            self.attack,
            self.release,
            self.octave,
            self.vel_sens,
            self.key_lower,
            self.key_upper,
            self.fade_lower,
            self.fade_upper,
            self.mute as u8,
            self.dry,
            self.chorus,
            self.reverb,
            self.out_assign,
            self.out_mfx,
            self.decay,
            self.vib_rate,
            self.vib_depth,
            self.vib_delay,
        ]
    }
}

/// エフェクトと全パート共通の設定。
#[derive(Clone, PartialEq, Debug)]
pub struct Common {
    /// System Common 00-03 Master Tune 24-2024 (-100.0 - +100.0 cent)。
    pub master_tune: u16,
    /// 04 Master Key Shift 40-88 (-24 - +24)。
    pub key_shift: u8,
    /// 05 Master Level 0-127。
    pub master_level: u8,

    /// System EQ。`[L, R]` それぞれ (Low Freq 0-1, Low Gain 0-30,
    /// High Freq 0-2, High Gain 0-30)。
    pub eq_on: bool,
    pub eq: [[u8; 4]; 2],

    /// Multitimbre Common 0C Solo Part Select 0 = OFF, 1-32。
    pub solo: u8,
    /// 30-32 MFX1-3 Source, 33 Chorus Source, 34 Reverb Source。0 = COMMON, 1-32。
    pub mfx_source: [u8; 3],
    pub chorus_source: u8,
    pub reverb_source: u8,

    /// Native: Chorus Type 0-3 (OFF, CHORUS, DELAY, GM2-CHORUS)。
    pub chorus_type: u8,
    pub chorus_level: u8,
    /// Chorus Output Select 0 MAIN, 1 REV, 2 MAIN+REV。
    pub chorus_out: u8,
    /// Native: Reverb Type 0-5 (OFF, REVERB, SRV-ROOM, SRV-HALL, SRV-PLATE, GM2-REVERB)。
    pub reverb_type: u8,
    pub reverb_level: u8,

    /// MFX1-3: (Type 0-90, Dry Send, Chorus Send, Reverb Send)。
    pub mfx: [[u8; 4]; 3],

    /// GM2: Reverb Type (00 Small Room ... 08 Plate) と Time。
    pub gm2_reverb_type: u8,
    pub gm2_reverb_time: u8,
    /// GM2: Chorus Type 0-5 と Mod Rate / Mod Depth / Feedback / Send To Reverb。
    pub gm2_chorus_type: u8,
    pub gm2_chorus: [u8; 4],
}

/// GM2 の Reverb Type の値と名前 (MI p.3)。6, 7 は欠番。
pub const GM2_REVERB_TYPES: [(u8, &str); 6] = [
    (0, "Small Room"),
    (1, "Medium Room"),
    (2, "Large Room"),
    (3, "Medium Hall"),
    (4, "Large Hall"),
    (8, "Plate"),
];
/// Reverb Type ごとの Reverb Time の既定値 (Owner's Manual p.40)。
pub fn gm2_reverb_time_default(t: u8) -> u8 {
    match t {
        0 => 44,
        1 => 50,
        2 => 56,
        3 | 4 => 64,
        _ => 50,
    }
}
pub const GM2_CHORUS_TYPES: [&str; 6] = [
    "Chorus 1",
    "Chorus 2",
    "Chorus 3",
    "Chorus 4",
    "FB Chorus",
    "Flanger",
];
/// Chorus Type ごとの (Mod Rate, Mod Depth, Feedback, Send To Reverb) の既定値 (p.40)。
pub fn gm2_chorus_default(t: u8) -> [u8; 4] {
    match t {
        0 => [3, 5, 0, 0],
        1 => [9, 19, 5, 0],
        2 => [3, 19, 8, 0],
        3 => [9, 16, 16, 0],
        4 => [2, 24, 64, 0],
        _ => [1, 5, 112, 0],
    }
}
pub const REVERB_TYPES: [&str; 6] = ["OFF", "REVERB", "SRV-ROOM", "SRV-HALL", "SRV-PLATE", "GM2-REVERB"];
pub const CHORUS_TYPES: [&str; 4] = ["OFF", "CHORUS", "DELAY", "GM2-CHORUS"];
pub const CHORUS_OUTS: [&str; 3] = ["MAIN", "REV", "MAIN+REV"];
pub const EQ_LOW_FREQS: [&str; 2] = ["200 Hz", "400 Hz"];
pub const EQ_HIGH_FREQS: [&str; 3] = ["2000 Hz", "4000 Hz", "8000 Hz"];

impl Common {
    pub fn initial(mode: Mode) -> Common {
        let gm2 = mode == Mode::Gm2;
        Common {
            master_tune: 1024,
            key_shift: 64,
            master_level: 127,
            eq_on: true,
            eq: [[0, 15, 0, 15]; 2],
            solo: 0,
            mfx_source: [1, 2, 3],
            chorus_source: 0,
            reverb_source: 0,
            // GM2 モードの初期化後は GM2 のタイプが選ばれている前提で持つ。
            chorus_type: if gm2 { 3 } else { 1 },
            chorus_level: 127,
            chorus_out: 0,
            reverb_type: if gm2 { 5 } else { 3 },
            reverb_level: 100,
            mfx: [[0, 127, 0, 0]; 3],
            gm2_reverb_type: 4,
            gm2_reverb_time: gm2_reverb_time_default(4),
            gm2_chorus_type: 2,
            gm2_chorus: gm2_chorus_default(2),
        }
    }
}

/// エディタが持つ本体の状態一式。
#[derive(Clone, PartialEq, Debug)]
pub struct Model {
    pub mode: Mode,
    /// Device ID (Owner's Manual p.72)。既定 17 = 10H。
    pub device_id: u8,
    pub common: Common,
    pub parts: Vec<Part>,
    /// プロジェクトを開いたとき / 本体を掴んだときに、この状態を本体へ
    /// 送り直すか。エディタで何か触ったら立つ。触っていなければ DAW 側の
    /// データ (曲頭の GM リセット等) に任せて何も送らない。
    pub recall: bool,
}

impl Default for Model {
    fn default() -> Self {
        Model::initial(Mode::Gm2)
    }
}

impl Model {
    pub fn initial(mode: Mode) -> Model {
        Model {
            mode,
            device_id: 0x10,
            common: Common::initial(mode),
            parts: (0..PARTS).map(|i| Part::initial(mode, i)).collect(),
            recall: false,
        }
    }

    /// 音源モードを切り替える。本体は初期化されるので、こちらも初期値に戻す。
    pub fn switch_mode(&mut self, mode: Mode) {
        let id = self.device_id;
        *self = Model::initial(mode);
        self.device_id = id;
        self.recall = true;
    }
}

// ---- 送るもの -------------------------------------------------------------

/// 本体に送る 1 単位。
#[derive(Clone, Copy)]
pub enum Out {
    /// 完全な MIDI メッセージ 1 個。
    Msg { cable: u8, len: u8, data: [u8; MAX_MSG] },
    /// 次を送るまで空ける時間 (ミリ秒)。モード切り替え直後などに使う。
    Wait(u16),
}

impl Out {
    pub fn msg(cable: u8, bytes: &[u8]) -> Out {
        let mut data = [0u8; MAX_MSG];
        let n = bytes.len().min(MAX_MSG);
        data[..n].copy_from_slice(&bytes[..n]);
        Out::Msg {
            cable,
            len: n as u8,
            data,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Out::Msg { len, data, .. } => &data[..*len as usize],
            Out::Wait(_) => &[],
        }
    }
}

/// Roland のチェックサム。アドレスとデータの和の下位 7 bit を 0 にする値。
fn checksum(bytes: &[u8]) -> u8 {
    let sum: u32 = bytes.iter().map(|&b| b as u32).sum();
    ((128 - (sum % 128)) % 128) as u8
}

/// Data Set 1 (DT1)。`F0 41 dev 00 48 12 aaaaaaaa data.. sum F7`。
pub fn dt1(device_id: u8, addr: [u8; 4], data: &[u8]) -> Out {
    let mut b = Vec::with_capacity(11 + data.len());
    b.extend_from_slice(&[0xF0, 0x41, device_id, MODEL_ID[0], MODEL_ID[1], 0x12]);
    b.extend_from_slice(&addr);
    b.extend_from_slice(data);
    b.push(checksum(&b[6..]));
    b.push(0xF7);
    debug_assert!(b.len() <= MAX_MSG);
    Out::msg(0, &b)
}

fn part_addr(index: usize, offset: u8) -> [u8; 4] {
    [0x10, 0x00, 0x20 + index as u8, offset]
}

/// GM2 Global Parameter Control (MI p.3)。`slot` は 1 = Reverb, 2 = Chorus。
fn gm2_gpc(slot: u8, param: u8, value: u8) -> Out {
    Out::msg(
        0,
        &[
            0xF0, 0x7F, 0x7F, 0x04, 0x05, 0x01, 0x01, 0x01, 0x01, slot, param, value, 0xF7,
        ],
    )
}

/// エディタ上の 1 回の変更。どこが変わったかだけを持ち、値は `Model` から引く。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// 音色 (Part Mode / セット / PC / Variation)。
    Tone(usize),
    /// パートの 1 アドレス (Multitimbre Part のオフセット)。
    Part(usize, u8),
    /// Modulation Depth (CC#1)。
    Modulation(usize),
    /// GS / XGlite で CC 経由にするもの (7, 10, 91, 93)。
    PartCc(usize, u8),
    MasterTune,
    KeyShift,
    MasterLevel,
    Eq,
    Solo,
    Sources,
    Chorus,
    Reverb,
    Mfx(usize),
    Gm2Reverb,
    Gm2Chorus,
}

impl Model {
    /// パートの 1 アドレスぶんのデータ。
    fn part_value(&self, index: usize, offset: u8) -> Vec<u8> {
        let p = &self.parts[index];
        match offset {
            0x00 => vec![p.rx_ch],
            0x01 => vec![p.rx_sw as u8],
            0x07..=0x24 => {
                let b = p.block_07_24();
                // 0F-10 は 2 ニブルで 1 つの値。どちらを指しても両方送る。
                match offset {
                    0x0F | 0x10 => b[(0x0F - 0x07)..=(0x10 - 0x07)].to_vec(),
                    o => vec![b[(o - 0x07) as usize]],
                }
            }
            0x33..=0x3E => vec![p.scale[(offset - 0x33) as usize]],
            0x3F => vec![p.set.gm2_index().unwrap_or(1) as u8],
            _ => vec![],
        }
    }

    /// 音色選択のメッセージ (p.79-83)。
    fn tone_messages(&self, index: usize, out: &mut Vec<Out>) {
        let p = &self.parts[index];
        let cable = Part::cable(index);
        let ch = p.rx_ch & 0x0F;
        let (msb, lsb) = match self.mode {
            Mode::Native => {
                let msb = if p.drum {
                    p.set.native_drum_msb().unwrap_or(104)
                } else {
                    p.set.native_inst_msb()
                };
                (msb, if p.drum { 0 } else { p.lsb })
            }
            Mode::Gm2 => {
                // 音色セットは DT1 で先に切り替える (p.79)。
                let set = p.set.gm2_index().unwrap_or(1) as u8;
                out.push(dt1(self.device_id, part_addr(index, 0x3F), &[set]));
                (if p.drum { 120 } else { 121 }, if p.drum { 0 } else { p.lsb })
            }
            // GS は LSB を 0 のまま (p.83)。
            Mode::Gs => (p.raw_msb, 0),
            Mode::XgLite => (p.raw_msb, p.lsb),
        };
        if !p.rx_sw {
            // 受信が切られているパートにはチャンネルで届かない。Native なら
            // アドレスで直接書く (Patch Bank Select MSB/LSB, Program Number)。
            if self.mode == Mode::Native {
                out.push(dt1(
                    self.device_id,
                    part_addr(index, 0x04),
                    &[msb, lsb, p.pc.saturating_sub(1)],
                ));
            }
            return;
        }
        let mut m = |b: &[u8]| out.push(Out::msg(cable, b));
        m(&[0xB0 | ch, 0x00, msb]);
        m(&[0xB0 | ch, 0x20, lsb]);
        m(&[0xC0 | ch, p.pc.saturating_sub(1) & 0x7F]);
    }

    /// 1 回の変更を本体に伝えるメッセージ。
    pub fn messages(&self, change: Change) -> Vec<Out> {
        let id = self.device_id;
        let c = &self.common;
        let mut out = Vec::new();
        match change {
            Change::Tone(i) => self.tone_messages(i, &mut out),
            Change::Part(i, off) => {
                let off = if off == 0x10 { 0x0F } else { off };
                let data = self.part_value(i, off);
                if !data.is_empty() {
                    out.push(dt1(id, part_addr(i, off), &data));
                }
            }
            Change::Modulation(i) => {
                let p = &self.parts[i];
                out.push(Out::msg(Part::cable(i), &[0xB0 | p.rx_ch, 0x01, p.modulation]));
            }
            Change::PartCc(i, cc) => {
                let p = &self.parts[i];
                let v = match cc {
                    7 => p.level,
                    10 => p.pan,
                    91 => p.reverb,
                    93 => p.chorus,
                    _ => return out,
                };
                out.push(Out::msg(Part::cable(i), &[0xB0 | p.rx_ch, cc, v]));
            }
            Change::MasterTune => {
                let t = c.master_tune;
                let n = [(t >> 12) as u8 & 0xF, (t >> 8) as u8 & 0xF, (t >> 4) as u8 & 0xF, t as u8 & 0xF];
                out.push(dt1(id, [0x01, 0x00, 0x00, 0x00], &n));
            }
            Change::KeyShift => out.push(dt1(id, [0x01, 0x00, 0x00, 0x04], &[c.key_shift])),
            Change::MasterLevel => out.push(dt1(id, [0x01, 0x00, 0x00, 0x05], &[c.master_level])),
            Change::Eq => {
                let mut d = vec![c.eq_on as u8];
                d.extend_from_slice(&c.eq[0]);
                d.extend_from_slice(&c.eq[1]);
                out.push(dt1(id, [0x01, 0x00, 0x02, 0x00], &d));
            }
            Change::Solo => out.push(dt1(id, [0x10, 0x00, 0x00, 0x0C], &[c.solo])),
            Change::Sources => {
                let d = [
                    c.mfx_source[0],
                    c.mfx_source[1],
                    c.mfx_source[2],
                    c.chorus_source,
                    c.reverb_source,
                ];
                out.push(dt1(id, [0x10, 0x00, 0x00, 0x30], &d));
            }
            Change::Chorus => {
                out.push(dt1(id, [0x10, 0x00, 0x02, 0x00], &[c.chorus_type, c.chorus_level]));
                out.push(dt1(id, [0x10, 0x00, 0x02, 0x03], &[c.chorus_out]));
            }
            Change::Reverb => {
                out.push(dt1(id, [0x10, 0x00, 0x04, 0x00], &[c.reverb_type, c.reverb_level]));
            }
            Change::Mfx(n) => {
                out.push(dt1(id, [0x10, 0x00, 0x06 + 2 * n as u8, 0x00], &c.mfx[n]));
            }
            Change::Gm2Reverb => {
                out.push(gm2_gpc(1, 0, c.gm2_reverb_type));
                out.push(gm2_gpc(1, 1, c.gm2_reverb_time));
            }
            Change::Gm2Chorus => {
                out.push(gm2_gpc(2, 0, c.gm2_chorus_type));
                for (k, &v) in c.gm2_chorus.iter().enumerate() {
                    out.push(gm2_gpc(2, k as u8 + 1, v));
                }
            }
        }
        out
    }

    /// `self` から `new` への変更を、送るべき単位に分ける。モードは変えないこと
    /// (モードの切り替えは `switch_mode` と `Mode::reset_message`)。
    pub fn diff(&self, new: &Model) -> Vec<Change> {
        let mut ch = Vec::new();
        let editable = new.mode.editable();
        for (i, (a, b)) in self.parts.iter().zip(&new.parts).enumerate() {
            if (a.drum, a.set, a.pc, a.lsb, a.raw_msb) != (b.drum, b.set, b.pc, b.lsb, b.raw_msb) {
                ch.push(Change::Tone(i));
            }
            if a.modulation != b.modulation {
                ch.push(Change::Modulation(i));
            }
            if !editable {
                for (cc, x, y) in [
                    (7, a.level, b.level),
                    (10, a.pan, b.pan),
                    (91, a.reverb, b.reverb),
                    (93, a.chorus, b.chorus),
                ] {
                    if x != y {
                        ch.push(Change::PartCc(i, cc));
                    }
                }
                continue;
            }
            if a.rx_ch != b.rx_ch {
                ch.push(Change::Part(i, 0x00));
            }
            if a.rx_sw != b.rx_sw {
                ch.push(Change::Part(i, 0x01));
            }
            let (ba, bb) = (a.block_07_24(), b.block_07_24());
            for k in 0..ba.len() {
                let off = 0x07 + k as u8;
                if ba[k] != bb[k] && off != 0x10 {
                    ch.push(Change::Part(i, off));
                } else if ba[k] != bb[k] && !ch.contains(&Change::Part(i, 0x0F)) {
                    ch.push(Change::Part(i, 0x0F));
                }
            }
            for k in 0..12 {
                if a.scale[k] != b.scale[k] {
                    ch.push(Change::Part(i, 0x33 + k as u8));
                }
            }
        }
        if !editable {
            return ch;
        }
        let (a, b) = (&self.common, &new.common);
        if a.master_tune != b.master_tune {
            ch.push(Change::MasterTune);
        }
        if a.key_shift != b.key_shift {
            ch.push(Change::KeyShift);
        }
        if a.master_level != b.master_level {
            ch.push(Change::MasterLevel);
        }
        if (a.eq_on, a.eq) != (b.eq_on, b.eq) {
            ch.push(Change::Eq);
        }
        if a.solo != b.solo {
            ch.push(Change::Solo);
        }
        if (a.mfx_source, a.chorus_source, a.reverb_source)
            != (b.mfx_source, b.chorus_source, b.reverb_source)
        {
            ch.push(Change::Sources);
        }
        if (a.chorus_type, a.chorus_level, a.chorus_out) != (b.chorus_type, b.chorus_level, b.chorus_out) {
            ch.push(Change::Chorus);
        }
        if (a.reverb_type, a.reverb_level) != (b.reverb_type, b.reverb_level) {
            ch.push(Change::Reverb);
        }
        for n in 0..3 {
            if a.mfx[n] != b.mfx[n] {
                ch.push(Change::Mfx(n));
            }
        }
        if (a.gm2_reverb_type, a.gm2_reverb_time) != (b.gm2_reverb_type, b.gm2_reverb_time) {
            ch.push(Change::Gm2Reverb);
        }
        if (a.gm2_chorus_type, a.gm2_chorus) != (b.gm2_chorus_type, b.gm2_chorus) {
            ch.push(Change::Gm2Chorus);
        }
        ch
    }

    /// 状態一式を本体に送り直すメッセージ。モード切り替えから始めるので、
    /// 本体側の状態に関わらずこの状態に揃う。
    pub fn dump(&self) -> Vec<Out> {
        let mut out = vec![self.mode.reset(self.device_id)];
        // 初期化を待つ。GM System On の後は 50ms 程度空けるのが通例だが、
        // 32 パートぶん初期化する SD-90 には余裕を持たせる。
        out.push(Out::Wait(300));
        let init = Model::initial(self.mode);

        if self.mode.editable() {
            let ci = &init.common;
            let c = &self.common;
            if c.master_tune != ci.master_tune {
                out.extend(self.messages(Change::MasterTune));
            }
            if c.key_shift != ci.key_shift {
                out.extend(self.messages(Change::KeyShift));
            }
            if c.master_level != ci.master_level {
                out.extend(self.messages(Change::MasterLevel));
            }
            if (c.eq_on, c.eq) != (ci.eq_on, ci.eq) {
                out.extend(self.messages(Change::Eq));
            }
            if (c.mfx_source, c.chorus_source, c.reverb_source)
                != (ci.mfx_source, ci.chorus_source, ci.reverb_source)
            {
                out.extend(self.messages(Change::Sources));
            }
            if (c.chorus_type, c.chorus_level, c.chorus_out)
                != (ci.chorus_type, ci.chorus_level, ci.chorus_out)
            {
                out.extend(self.messages(Change::Chorus));
            }
            if (c.reverb_type, c.reverb_level) != (ci.reverb_type, ci.reverb_level) {
                out.extend(self.messages(Change::Reverb));
            }
            if self.mode == Mode::Native {
                for n in 0..3 {
                    if c.mfx[n] != ci.mfx[n] {
                        out.extend(self.messages(Change::Mfx(n)));
                    }
                }
            }
            if self.mode == Mode::Gm2 {
                if (c.gm2_reverb_type, c.gm2_reverb_time) != (ci.gm2_reverb_type, ci.gm2_reverb_time) {
                    out.extend(self.messages(Change::Gm2Reverb));
                }
                if (c.gm2_chorus_type, c.gm2_chorus) != (ci.gm2_chorus_type, ci.gm2_chorus) {
                    out.extend(self.messages(Change::Gm2Chorus));
                }
            }
        }

        for (i, (p, pi)) in self.parts.iter().zip(&init.parts).enumerate() {
            if !self.mode.editable() {
                if (p.raw_msb, p.lsb, p.pc) != (pi.raw_msb, pi.lsb, pi.pc) {
                    self.tone_messages(i, &mut out);
                }
                for cc in [7, 10, 91, 93] {
                    out.extend(self.messages(Change::PartCc(i, cc)));
                }
                continue;
            }
            if (p.rx_ch, p.rx_sw) != (pi.rx_ch, pi.rx_sw) {
                out.push(dt1(self.device_id, part_addr(i, 0x00), &[p.rx_ch, p.rx_sw as u8]));
            }
            if (p.drum, p.set, p.pc, p.lsb) != (pi.drum, pi.set, pi.pc, pi.lsb) {
                self.tone_messages(i, &mut out);
            }
            let (b, bi) = (p.block_07_24(), pi.block_07_24());
            if b != bi {
                out.push(dt1(self.device_id, part_addr(i, 0x07), &b));
            }
            if p.scale != pi.scale {
                out.push(dt1(self.device_id, part_addr(i, 0x33), &p.scale));
            }
            if p.modulation != pi.modulation {
                out.extend(self.messages(Change::Modulation(i)));
            }
        }
        if self.mode.editable() && self.common.solo != 0 {
            out.extend(self.messages(Change::Solo));
        }
        out
    }
}

// ---- 保存 -----------------------------------------------------------------
//
// プロジェクトに埋める形式。先頭 4 バイトの印と版のあとに値を素直に並べる。
// 版を上げるときは読み側で古い版も読めるようにすること。

const MAGIC: &[u8; 4] = b"SD9E";
const VERSION: u8 = 1;

impl Model {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(2048);
        b.extend_from_slice(MAGIC);
        b.push(VERSION);
        b.push(self.mode.to_u8());
        b.push(self.device_id);
        b.push(self.recall as u8);
        let c = &self.common;
        b.extend_from_slice(&c.master_tune.to_le_bytes());
        b.extend_from_slice(&[c.key_shift, c.master_level, c.eq_on as u8]);
        b.extend_from_slice(&c.eq[0]);
        b.extend_from_slice(&c.eq[1]);
        b.push(c.solo);
        b.extend_from_slice(&c.mfx_source);
        b.extend_from_slice(&[c.chorus_source, c.reverb_source]);
        b.extend_from_slice(&[c.chorus_type, c.chorus_level, c.chorus_out]);
        b.extend_from_slice(&[c.reverb_type, c.reverb_level]);
        for m in &c.mfx {
            b.extend_from_slice(m);
        }
        b.extend_from_slice(&[c.gm2_reverb_type, c.gm2_reverb_time, c.gm2_chorus_type]);
        b.extend_from_slice(&c.gm2_chorus);
        b.push(self.parts.len() as u8);
        for p in &self.parts {
            let set = SoundSet::ALL.iter().position(|&s| s == p.set).unwrap_or(1) as u8;
            b.extend_from_slice(&[p.drum as u8, set, p.pc, p.lsb, p.raw_msb, p.rx_ch, p.rx_sw as u8]);
            b.extend_from_slice(&p.block_07_24());
            b.extend_from_slice(&p.scale);
            b.push(p.modulation);
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Model> {
        let mut r = Reader { b, at: 0 };
        if r.take(4)? != MAGIC || r.u8()? != VERSION {
            return None;
        }
        let mode = Mode::from_u8(r.u8()?);
        let mut m = Model::initial(mode);
        m.device_id = r.u8()? & 0x7F;
        m.recall = r.u8()? != 0;
        let c = &mut m.common;
        c.master_tune = u16::from_le_bytes([r.u8()?, r.u8()?]).clamp(24, 2024);
        c.key_shift = r.u8()?;
        c.master_level = r.u8()?;
        c.eq_on = r.u8()? != 0;
        c.eq[0].copy_from_slice(r.take(4)?);
        c.eq[1].copy_from_slice(r.take(4)?);
        c.solo = r.u8()?;
        c.mfx_source.copy_from_slice(r.take(3)?);
        c.chorus_source = r.u8()?;
        c.reverb_source = r.u8()?;
        c.chorus_type = r.u8()?;
        c.chorus_level = r.u8()?;
        c.chorus_out = r.u8()?;
        c.reverb_type = r.u8()?;
        c.reverb_level = r.u8()?;
        for n in 0..3 {
            c.mfx[n].copy_from_slice(r.take(4)?);
        }
        c.gm2_reverb_type = r.u8()?;
        c.gm2_reverb_time = r.u8()?;
        c.gm2_chorus_type = r.u8()?;
        c.gm2_chorus.copy_from_slice(r.take(4)?);
        let n = r.u8()? as usize;
        for i in 0..n.min(PARTS) {
            let p = &mut m.parts[i];
            p.drum = r.u8()? != 0;
            p.set = *SoundSet::ALL.get(r.u8()? as usize)?;
            p.pc = r.u8()?.clamp(1, 128);
            p.lsb = r.u8()?;
            p.raw_msb = r.u8()?;
            p.rx_ch = r.u8()? & 0x0F;
            p.rx_sw = r.u8()? != 0;
            let blk = r.take(0x24 - 0x07 + 1)?;
            p.level = blk[0];
            p.pan = blk[1];
            p.coarse = blk[2];
            p.fine = blk[3];
            p.mono_poly = blk[4];
            p.legato = blk[5];
            p.pb_range = blk[6];
            p.porta_sw = blk[7];
            p.porta_time = (blk[8] << 4) | (blk[9] & 0x0F);
            p.cutoff = blk[10];
            p.resonance = blk[11];
            p.attack = blk[12];
            p.release = blk[13];
            p.octave = blk[14];
            p.vel_sens = blk[15];
            p.key_lower = blk[16];
            p.key_upper = blk[17];
            p.fade_lower = blk[18];
            p.fade_upper = blk[19];
            p.mute = blk[20] != 0;
            p.dry = blk[21];
            p.chorus = blk[22];
            p.reverb = blk[23];
            p.out_assign = blk[24];
            p.out_mfx = blk[25];
            p.decay = blk[26];
            p.vib_rate = blk[27];
            p.vib_depth = blk[28];
            p.vib_delay = blk[29];
            p.scale.copy_from_slice(r.take(12)?);
            p.modulation = r.u8()?;
        }
        Some(m)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_manual() {
        // p.77 の Native On: F0 41 10 00 48 12 00 00 00 00 00 00 F7
        assert_eq!(dt1(0x10, [0, 0, 0, 0], &[0]).bytes(), Mode::Native.reset_message());
        // p.79 の例: Part 1 の音色セットを Enhanced に。10 00 20 3F 03 -> sum 0x72 -> 0x0E
        assert_eq!(
            dt1(0x10, [0x10, 0x00, 0x20, 0x3F], &[0x03]).bytes(),
            &[0xF0, 0x41, 0x10, 0x00, 0x48, 0x12, 0x10, 0x00, 0x20, 0x3F, 0x03, 0x0E, 0xF7]
        );
    }

    #[test]
    fn roundtrip() {
        let mut m = Model::initial(Mode::Native);
        m.parts[5].pc = 42;
        m.parts[5].porta_time = 99;
        m.parts[31].mute = true;
        m.common.mfx[2] = [17, 100, 20, 30];
        m.recall = true;
        assert_eq!(Model::from_bytes(&m.to_bytes()), Some(m));
    }

    #[test]
    fn dump_of_initial_state_is_only_reset() {
        let m = Model::initial(Mode::Gm2);
        let d = m.dump();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].bytes(), Mode::Gm2.reset_message());
    }

    #[test]
    fn edit_becomes_one_dt1() {
        let a = Model::initial(Mode::Native);
        let mut b = a.clone();
        b.parts[0].level = 0x50;
        let ch = a.diff(&b);
        assert_eq!(ch, vec![Change::Part(0, 0x07)]);
        // 10 00 20 07 50: 和 0x87 -> チェックサム 0x79
        assert_eq!(
            b.messages(ch[0])[0].bytes(),
            &[0xF0, 0x41, 0x10, 0x00, 0x48, 0x12, 0x10, 0x00, 0x20, 0x07, 0x50, 0x79, 0xF7]
        );
        // Portamento Time は 2 ニブルを一緒に送る。
        let mut c = a.clone();
        c.parts[3].porta_time = 0x35;
        let ch = a.diff(&c);
        assert_eq!(ch, vec![Change::Part(3, 0x0F)]);
        assert_eq!(&c.messages(ch[0])[0].bytes()[6..12], &[0x10, 0x00, 0x23, 0x0F, 0x03, 0x05]);
    }

    #[test]
    fn tone_select_follows_manual() {
        // Native: Bank MSB がセット (p.83)。B02 = Port B, ch 2。
        let mut m = Model::initial(Mode::Native);
        let p = &mut m.parts[17];
        p.set = SoundSet::Special2;
        p.pc = 5;
        let out = m.messages(Change::Tone(17));
        let got: Vec<(u8, Vec<u8>)> = out
            .iter()
            .map(|o| match o {
                Out::Msg { cable, .. } => (*cable, o.bytes().to_vec()),
                Out::Wait(_) => (9, vec![]),
            })
            .collect();
        assert_eq!(
            got,
            vec![(1, vec![0xB1, 0x00, 81]), (1, vec![0xB1, 0x20, 0]), (1, vec![0xC1, 4])]
        );
        // GM2: セットは DT1 (10 00 pp 3F) で先に、Bank MSB は 121 / 120 (p.79, p.83)。
        let mut g = Model::initial(Mode::Gm2);
        g.parts[9].set = SoundSet::Solo;
        g.parts[9].pc = 33;
        let out = g.messages(Change::Tone(9));
        assert_eq!(&out[0].bytes()[6..11], &[0x10, 0x00, 0x29, 0x3F, 0x02]);
        assert_eq!(out[1].bytes(), &[0xB9, 0x00, 120]);
        assert_eq!(out[3].bytes(), &[0xC9, 32]);
    }

    #[test]
    fn reset_uses_device_id() {
        assert_eq!(Mode::Native.reset(0x11).bytes()[2], 0x11);
        assert_eq!(Mode::Gm2.reset(0x11).bytes(), Mode::Gm2.reset_message());
    }

    #[test]
    fn part_block_fits() {
        let m = Model::initial(Mode::Native);
        assert!(dt1(0x10, part_addr(0, 0x07), &m.parts[0].block_07_24()).bytes().len() <= MAX_MSG);
    }
}
