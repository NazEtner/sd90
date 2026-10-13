//! スタンダード MIDI ファイルを「秒つきの送出イベント列」に落とす。
//!
//! 演奏に要るのは「何秒後にこのバイト列を出すか」だけなので、トラック構造は
//! 読んだ時点で潰して 1 本に畳む。テンポ変更はここで秒に織り込んでしまうので、
//! 送出側はテンポを知らなくてよい。
//!
//! ランニングステータス (ステータスバイト省略) は 2002 年ごろのシーケンサが
//! 普通に吐くので必須。東方紅魔郷のデータも使っている。

/// 送出単位。ショートメッセージと SysEx で Windows 側の API が別なので分けておく。
#[derive(Clone, Debug)]
pub enum Msg {
    /// ステータス + データ 0〜2 バイト。
    Short(Vec<u8>),
    /// F0 から F7 までを含んだ完全な SysEx。
    Sysex(Vec<u8>),
}

impl Msg {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Msg::Short(m) | Msg::Sysex(m) => m,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub sec: f64,
    pub msg: Msg,
}

pub struct Song {
    pub name: String,
    pub division: u16,
    pub events: Vec<Event>,
    /// 最後のイベントの時刻 (秒)。
    pub duration: f64,
    /// テンポ変更の一覧 (秒, BPM)。表示用。
    pub tempos: Vec<(f64, f64)>,
}

fn be16(b: &[u8]) -> u16 {
    ((b[0] as u16) << 8) | b[1] as u16
}
fn be32(b: &[u8]) -> u32 {
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | b[3] as u32
}

fn var(b: &[u8], i: &mut usize) -> Result<u64, String> {
    let mut v: u64 = 0;
    for _ in 0..4 {
        let c = *b.get(*i).ok_or("可変長数値の途中でトラックが終わった")?;
        *i += 1;
        v = (v << 7) | (c & 0x7F) as u64;
        if c & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err("可変長数値が長すぎる".into())
}

/// トラック 1 本を (tick, 生イベント) に開く。メタは種類とデータのまま返す。
fn parse_track(t: &[u8]) -> Result<Vec<(u64, Result<Msg, (u8, Vec<u8>)>)>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut tick = 0u64;
    let mut status = 0u8;
    while i < t.len() {
        tick += var(t, &mut i)?;
        let mut c = *t.get(i).ok_or("トラックが途中で切れている")?;
        if c & 0x80 != 0 {
            i += 1;
            // システムメッセージはランニングステータスを打ち切る。
            status = if c >= 0xF0 { 0 } else { c };
        } else {
            // ランニングステータス。直前のステータスを使い回す。
            c = status;
        }
        match c {
            0xFF => {
                let kind = t[i];
                i += 1;
                let n = var(t, &mut i)? as usize;
                out.push((tick, Err((kind, t[i..i + n].to_vec()))));
                i += n;
            }
            0xF0 => {
                let n = var(t, &mut i)? as usize;
                let mut m = Vec::with_capacity(n + 1);
                m.push(0xF0);
                m.extend_from_slice(&t[i..i + n]);
                i += n;
                out.push((tick, Ok(Msg::Sysex(m))));
            }
            0xF7 => {
                // エスケープ。中身をそのまま流す (分割 SysEx の続き)。
                let n = var(t, &mut i)? as usize;
                out.push((tick, Ok(Msg::Sysex(t[i..i + n].to_vec()))));
                i += n;
            }
            _ => {
                let n = match c & 0xF0 {
                    0xC0 | 0xD0 => 1,
                    _ => 2,
                };
                let mut m = Vec::with_capacity(n + 1);
                m.push(c);
                m.extend_from_slice(t.get(i..i + n).ok_or("イベントが途中で切れている")?);
                i += n;
                out.push((tick, Ok(Msg::Short(m))));
            }
        }
    }
    Ok(out)
}

pub fn load(path: &str) -> Result<Song, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 14 || &b[0..4] != b"MThd" {
        return Err(format!("{path}: MThd が無い"));
    }
    let division = be16(&b[12..14]);
    if division & 0x8000 != 0 {
        return Err("SMPTE タイムコードのファイルには未対応".into());
    }

    let mut tracks = Vec::new();
    let mut i = 14usize;
    while i + 8 <= b.len() {
        let len = be32(&b[i + 4..i + 8]) as usize;
        if &b[i..i + 4] == b"MTrk" {
            tracks.push(parse_track(&b[i + 8..(i + 8 + len).min(b.len())])?);
        }
        i += 8 + len;
    }

    // 全トラックを tick 順に畳む。同 tick は元のトラック順・出現順を保つ
    // (プログラムチェンジとノートの前後関係が壊れると音色が変わる)。
    let mut all: Vec<(u64, usize, Result<Msg, (u8, Vec<u8>)>)> = Vec::new();
    for (ti, tr) in tracks.into_iter().enumerate() {
        for (tick, e) in tr {
            all.push((tick, ti, e));
        }
    }
    all.sort_by_key(|(tick, ti, _)| (*tick, *ti));

    // テンポを織り込みながら秒に直す。
    let mut events = Vec::with_capacity(all.len());
    let mut tempos = Vec::new();
    let mut name = String::new();
    let mut sec = 0.0f64;
    let mut last_tick = 0u64;
    let mut us_per_beat = 500_000.0f64;
    for (tick, _, e) in all {
        sec += (tick - last_tick) as f64 / division as f64 * us_per_beat / 1e6;
        last_tick = tick;
        match e {
            Ok(msg) => events.push(Event { sec, msg }),
            Err((kind, data)) => match kind {
                0x51 if data.len() == 3 => {
                    us_per_beat =
                        (((data[0] as u32) << 16) | ((data[1] as u32) << 8) | data[2] as u32) as f64;
                    tempos.push((sec, 60e6 / us_per_beat));
                }
                // トラック名は CP932 のことが多い。ここでは変換器を持たないので
                // UTF-8 として読めたときだけ拾う (表示用途にしか使わない)。
                0x03 if name.is_empty() => {
                    if let Ok(t) = String::from_utf8(data) {
                        name = t.trim().to_string();
                    }
                }
                _ => {}
            },
        }
    }
    let duration = events.last().map(|e| e.sec).unwrap_or(0.0);
    Ok(Song {
        name,
        division,
        events,
        duration,
        tempos,
    })
}
