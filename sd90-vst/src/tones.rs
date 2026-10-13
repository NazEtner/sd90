//! SD-90 の音色表。Owner's Manual の付録から起こしたもの。
//!
//! * "Instrument list (GM2 / Native mode)" (p.129-133)
//! * "Instrument list (Special sound)" (p.134)
//! * "Drum set list (GM2 / Native mode)" (p.139)
//!
//! 表記 (大文字小文字や綴りの揺れ、"Bandneon" "Hawaian" "Seqence" など) は
//! マニュアルの印字のまま。勝手に直すと本体の LCD と食い違う。
//!
//! Percussive / SFX (PC 113-128) は Contemporary / Solo / Enhanced 列が
//! マニュアル上で空欄 ("-") なので、`""` で持つ。

/// 音色セット。Native モードでは Bank Select MSB がそのままこれを選ぶ
/// (p.79 "Switching the sound set in Native mode")。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundSet {
    Classical,
    Contemporary,
    Solo,
    Enhanced,
    /// Native モードのみ。
    Special1,
    /// Native モードのみ。
    Special2,
}

impl SoundSet {
    pub const ALL: [SoundSet; 6] = [
        SoundSet::Classical,
        SoundSet::Contemporary,
        SoundSet::Solo,
        SoundSet::Enhanced,
        SoundSet::Special1,
        SoundSet::Special2,
    ];

    /// LCD の略記 (p.23)。
    pub fn short(self) -> &'static str {
        match self {
            SoundSet::Classical => "CLASIC",
            SoundSet::Contemporary => "CONTEM",
            SoundSet::Solo => "SOLO",
            SoundSet::Enhanced => "ENHANC",
            SoundSet::Special1 => "SP 1",
            SoundSet::Special2 => "SP 2",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SoundSet::Classical => "Classical",
            SoundSet::Contemporary => "Contemporary",
            SoundSet::Solo => "Solo",
            SoundSet::Enhanced => "Enhanced",
            SoundSet::Special1 => "Special 1",
            SoundSet::Special2 => "Special 2",
        }
    }

    /// GM2 / Native の 4 セットでの添字。Special は `None`。
    pub fn gm2_index(self) -> Option<usize> {
        match self {
            SoundSet::Classical => Some(0),
            SoundSet::Contemporary => Some(1),
            SoundSet::Solo => Some(2),
            SoundSet::Enhanced => Some(3),
            _ => None,
        }
    }

    pub fn from_gm2_index(i: u8) -> SoundSet {
        match i {
            0 => SoundSet::Classical,
            2 => SoundSet::Solo,
            3 => SoundSet::Enhanced,
            _ => SoundSet::Contemporary,
        }
    }

    pub fn is_special(self) -> bool {
        matches!(self, SoundSet::Special1 | SoundSet::Special2)
    }

    /// Native モードの Inst パートの Bank MSB (p.80)。
    pub fn native_inst_msb(self) -> u8 {
        match self {
            SoundSet::Special1 => 80,
            SoundSet::Special2 => 81,
            SoundSet::Classical => 96,
            SoundSet::Contemporary => 97,
            SoundSet::Solo => 98,
            SoundSet::Enhanced => 99,
        }
    }

    /// Native モードの Drum パートの Bank MSB (p.80)。Special にドラムは無い。
    pub fn native_drum_msb(self) -> Option<u8> {
        self.gm2_index().map(|i| 104 + i as u8)
    }

    /// Native モードの Bank MSB から (セット, ドラムか) を引く。
    pub fn from_native_msb(msb: u8) -> Option<(SoundSet, bool)> {
        Some(match msb {
            80 => (SoundSet::Special1, false),
            81 => (SoundSet::Special2, false),
            96..=99 => (SoundSet::from_gm2_index(msb - 96), false),
            104..=107 => (SoundSet::from_gm2_index(msb - 104), true),
            _ => return None,
        })
    }
}

/// 1 行 = (PC 1-128, Variation = Bank LSB, [Classical, Contemporary, Solo, Enhanced])。
pub const INST: &[(u8, u8, [&str; 4])] = &[
    // ---- Piano ----
    (1, 0, ["Piano 1", "Ac.Piano", "St.Piano 1", "SD Piano"]),
    (1, 1, ["Piano 1w", "Ac.Piano w", "St.Piano 1w", "SD Piano w"]),
    (1, 2, ["Piano 1d", "Mild Piano", "European Pf", "Classic Pf"]),
    (2, 0, ["Piano 2", "Rock Piano", "St.Piano 2", "Enh.Piano 2"]),
    (2, 1, ["Piano 2w", "Rock Piano w", "St.Piano 2w", "Enh.Piano 2w"]),
    (3, 0, ["Piano 3", "E.Grand Pf", "SA Piano", "Enh.E Grand"]),
    (3, 1, ["Piano 3w", "E.Grand Pf w", "SA Piano w", "Enh.E Grandw"]),
    (4, 0, ["Honky-tonk", "Old Honky", "St.Honky", "Brite Honky"]),
    (4, 1, ["Honky-tonk w", "Old Honky w", "St.Honky w", "Brite Honkyw"]),
    (5, 0, ["E.Piano 1", "Soft Rhodes", "Tremo Rhodes", "Stage 73"]),
    (5, 1, ["Detuned EP1", "Fat Rhodes", "Sweet Tynes", "NY Rhodes"]),
    (5, 2, ["Dyno Rhodes", "Rhodes Wide", "Tremo Dyno", "Phase Dyno"]),
    (5, 3, ["60's E.Piano", "Wurly Soft", "Tremo Wurly", "Whirly"]),
    (6, 0, ["E.Piano 2", "FM E.Piano", "FM Hard EP", "Chorus EP"]),
    (6, 1, ["Detuned EP2", "Soft FM EP", "Brite FM EP", "Phase FM EP"]),
    (6, 2, ["E.Piano 2v", "SA E.Piano", "Brite FM EP2", "Wah FM EP"]),
    (6, 3, ["EP Legend", "EP Legend 2", "EP Legend 3", "Enh.Legend"]),
    (6, 4, ["EP Phase", "EP Phase 2", "EP Phase 3", "Phasing EP"]),
    (7, 0, ["Harpsichord", "Harpsi 2", "St.Harpsichd", "Enh.Harpsi"]),
    (7, 1, ["Coupl hps", "Coupl hps 2", "St.Coupl hps", "Enh.Coupl hp"]),
    (7, 2, ["Harpsi w", "Harpsi 2 w", "St.Harpsi w", "Enh.Harpsi w"]),
    (7, 3, ["Harpsi o", "Harpsi 2 o", "St.Harpsi o", "Enh.Harpsi o"]),
    (8, 0, ["Clav", "Atack Clav 1", "Atack Clav 2", "Comp Clav"]),
    (8, 1, ["Pulse Clav", "AnalogClav 1", "AnalogClav 2", "Wah Ana.Clav"]),
    // ---- Chromatic percussion ----
    (9, 0, ["Celesta", "Celesta 2", "St.Celesta", "SpaceCelesta"]),
    (10, 0, ["Glockenspiel", "Glocken 2", "St.Glocken", "Trem.Glocken"]),
    (11, 0, ["Music Box", "Music Box 2", "St.Music Box", "Panning Box"]),
    (12, 0, ["Vibraphone", "Vibraphone 2", "St.Vibra", "Trem.Vibra"]),
    (12, 1, ["Vibraphone w", "Vibraphone2w", "St.Vibra w", "Trem.Vibra w"]),
    (13, 0, ["Marimba", "Marimba 2", "St.Marimba", "Enh.Marimba"]),
    (13, 1, ["Marimba w", "Marimba 2 w", "St.Marimba w", "Enh.Marimbaw"]),
    (14, 0, ["Xylophone", "Xylophone 2", "St.Xylophone", "Enh.Xylophon"]),
    (15, 0, ["Tubular-bell", "Tubular-bel2", "St.Tubular", "Trem.Tubula"]),
    (15, 1, ["Church Bell", "Church Bell2", "St.Church", "Echo Church"]),
    (15, 2, ["Carillon", "Carillon 2", "St.Carillon", "Trm.Carillon"]),
    (16, 0, ["Santur", "Santur 2", "St.Santur", "Enh.Santur"]),
    // ---- Organ ----
    (17, 0, ["Organ 1", "Perky", "Roller", "Perky Spin"]),
    (17, 1, ["Detuned Or1", "Ballad B", "Rocker", "Gospel Spin"]),
    (17, 2, ["Organ 60", "Happy 60s", "Soft60'Organ", "96 Year"]),
    (17, 3, ["Organ 4", "Tone Wheel", "Full Stops", "Tone Wh.Solo"]),
    (18, 0, ["Organ 2", "Jazz Organ 1", "Jazz Organ 2", "Jazzy Spin"]),
    (18, 1, ["Detuned Or2", "Perc.Organ 1", "Perc.Organ 2", "Jazzy Spin 2"]),
    (18, 2, ["Organ 5", "Dist.JzOrg 1", "Dist.JzOrg 2", "Jazzy Spin 3"]),
    (19, 0, ["Organ 3", "Organ 3 fast", "Rock Organ", "Rocker Spin"]),
    (20, 0, ["Church 1", "Pipe Organ 1", "Pipe Organ 2", "Amb.Church"]),
    (20, 1, ["Church 2", "LargeChurch1", "LargeChurch2", "Amb.Church 2"]),
    (20, 2, ["Church 3", "SmallChurch1", "SmallChurch2", "Amb.Church 3"]),
    (21, 0, ["Reed Organ", "Reed Organ 2", "Reed Organ 3", "Old Reed Org"]),
    (21, 1, ["Puff Organ", "Organ Flute", "Theater", "Enh.Theater"]),
    (22, 0, ["Accordion F", "French Acc", "St.FrenchAcc", "Enh.French"]),
    (22, 1, ["Accordion I", "It Muset", "St.It Muset", "Enh.ItMuset"]),
    (23, 0, ["Harmonica", "Harmonica 2", "St.Harmonica", "Ld.Harmonica"]),
    (24, 0, ["Bandneon", "Bandneon 1", "St.Bandneon", "Enh.Bandneon"]),
    // ---- Guitar ----
    (25, 0, ["Nylon Gt", "Nylon Gt 2", "Nylon Gt 3", "Enh.Nylon Gt"]),
    (25, 1, ["Ukulele", "Ukulele 2", "Ukulele 3", "Enh.Ukulele"]),
    (25, 2, ["Nylon o", "Nylon 2 o", "Nylon 3 o", "Enh.Nylon o"]),
    (25, 3, ["Nylon Gt.2", "Hard Gut Gt", "Hard Gut Gt2", "Enh.Gut Gt"]),
    (26, 0, ["Steel-str.Gt", "OV Steel Gt", "SteelStr.Gt2", "Comp OVSteel"]),
    (26, 1, ["12-Str.Gt", "12-Str.Gt 2", "12-Str.Gt 3", "3D 12-Str.Gt"]),
    (26, 2, ["Mandolin", "Mandolin 2", "Mandolin 3", "Enh.Mandolin"]),
    (26, 3, ["Steel+Body", "Steel+Body 2", "Steel+Body 3", "DelayedSteel"]),
    (27, 0, ["Jazz Gt", "Jazz Gt 2", "Jazz Gt 3", "Lead Jazz Gt"]),
    (27, 1, ["Pedal Steel", "Pedal Steel2", "Pedal Steel3", "Hawaian Gt"]),
    (28, 0, ["Clean Rear", "TC Rear", "Strat2 Rear", "Old Clean Gt"]),
    (28, 1, ["Clean Half", "TC Front", "Chorus Clean", "Jazz Chorus"]),
    (28, 2, ["Mid Tone Gt", "TC Front 2", "335", "335 Drive"]),
    (29, 0, ["Muted Gt", "TC Mute Gt", "TC Mute Gt 2", "Comp Mute Gt"]),
    (29, 1, ["Funk Gt", "FunkGt Slap", "FunkGt.Slap2", "Enh.Funk Gt"]),
    (29, 2, ["Funk Gt 2", "Funk Pop", "Funk Pop 2", "Wah Funk Pop"]),
    (29, 3, ["Jazz Man", "Mute Jazz Gt", "Slap Jazz Gt", "Solo Jazz Gt"]),
    (30, 0, ["Overdrive Gt", "Atk Drive Gt", "OverdriveGt2", "TC Lead Gt"]),
    (30, 1, ["Gt.Pinch", "Gt.Pinch 2", "Gt.Pinch 3", "Gt.PinchLead"]),
    (31, 0, ["DistortionGt", "Atk Dist Gt", "Dist.Gt 2", "Heavy DistGt"]),
    (31, 1, ["Feedback Gt", "FeedbackGt 2", "Feedback OD", "Feedbacker"]),
    (31, 2, ["DistRythm Gt", "Muted Dist", "Muted Dist 2", "Muted OD"]),
    (32, 0, ["Gt.Harmonics", "Gt.Harm 2", "Gt.OctHarm", "Amp.Harm"]),
    (32, 1, ["Gt.Feedback", "FeedbackOct", "FeedbackHarm", "Amp.FeedBack"]),
    // ---- Bass ----
    (33, 0, ["Acoustic Bs", "Rockabilly", "Fat Aco.Bass", "Enh.Aco Bass"]),
    (34, 0, ["Fingered Bs", "Fingered Bs2", "Jazz Bass", "Pre Bass"]),
    (34, 1, ["FingerJ.Bass", "FingerP.Bass", "Finger Slap", "Comp Finger"]),
    (35, 0, ["Picked Bass", "Picked Jz Bs", "Picking Bass", "Rock Bass"]),
    (36, 0, ["Fretless Bs", "Fretless Bs2", "PhaseFrtless", "Cho.Fretless"]),
    (37, 0, ["Slap Bass 1", "Slap Pop 1", "Jazz Slap", "Phase Slap"]),
    (38, 0, ["Slap Bass 2", "Funky Slap", "Slap Pop 2", "Enh.Slap Pop"]),
    (39, 0, ["Synth Bass 1", "MG303 Bass", "Fat Syn.Bass", "Dist303 Bass"]),
    (39, 1, ["SynthBass101", "MG Bass", "SynthSaw Bs", "P.Shift Bass"]),
    (39, 2, ["Acid Bass", "MG Acid Bass", "AcidBs Dirty", "Acid Dist Bs"]),
    (39, 3, ["Clavi Bass", "Clavi Bass 2", "Clavi Bass 3", "PhaseClaviBs"]),
    (39, 4, ["Hammer", "OB Hammer", "MG Hammer", "Enh.Hammer"]),
    (40, 0, ["Synth Bass 2", "Seq101 Bass", "Sq SynthBass", "PhaseSq Bass"]),
    (40, 1, ["Beef FM Bs", "Beef Slap Bs", "Beef Saw Bs", "Enh.Beef Bs"]),
    (40, 2, ["Rubber Bass", "Rubber Bass2", "JpSaw Rubber", "Fat JpSaw Bs"]),
    (40, 3, ["Attack Pulse", "Attack Saw", "Attack MG Bs", "Enh.MG Bass"]),
    // ---- Strings / orchestra ----
    (41, 0, ["Violin", "Violin vib", "Violin 2 vib", "Enh.Violin"]),
    (41, 1, ["Slow Violin", "Slow Vln vib", "SlowVln2 vib", "Enh.Slow Vln"]),
    (42, 0, ["Viola", "Viola vib", "Viola2 vib", "Enh.Viola"]),
    (43, 0, ["Cello", "Cello vib", "Cello2 vib", "Enh.Cello"]),
    (44, 0, ["Contrabass", "Cb vib", "Cb2 vib", "Enh.Cb"]),
    (45, 0, ["Tremolo Str", "Tremolo Str2", "St.Trem Str", "St.Trem Str2"]),
    (46, 0, ["Pizzicato", "Pizzicato 2", "St.Pizzicato", "Chorus Pizz"]),
    (47, 0, ["Harp", "Harp 2", "St.Harp", "Chorus Harp"]),
    (47, 1, ["Yangqin", "Yangqin 2", "St.Yangqin", "Enh.Yangqin"]),
    (48, 0, ["Timpani", "Timpani 2", "St.Timpani", "Enh.Timpani"]),
    // ---- Ensemble ----
    (49, 0, ["Strings", "Strings 2", "St.Strings", "St.Strings 2"]),
    (49, 1, ["Orchestra", "Orchestra 2", "St.Orchestra", "St.Orchestr2"]),
    (49, 2, ["60'Strings", "Oct.Strings", "St.OctStr 1", "St.OctStr 2"]),
    (50, 0, ["Slow Strings", "SlowStrings2", "St.Slow Str", "St.Slow Str2"]),
    (51, 0, ["Syn.Strings1", "BriteSyn.Str", "StackSyn.Str", "JP Strings"]),
    (51, 1, ["Syn.Strings3", "Oct.SynStr 1", "Oct.SynStr 2", "PhaseSyn.Str"]),
    (52, 0, ["Syn.Strings2", "Warm SynStr1", "Warm SynStr2", "OB Strings"]),
    (53, 0, ["Choir Aahs", "Large Choir", "St.ChoirAahs", "Rich Choir"]),
    (53, 1, ["Choir Aahs 2", "Small Choir", "St.Sm Choir", "St.Sm Choir2"]),
    (54, 0, ["Voices Oohs", "Voices Oohs 2", "St.Vox Oohs", "Enh.Vox Oohs"]),
    (54, 1, ["Humming", "Humming 2", "St.Humming", "Enh.Humming"]),
    (55, 0, ["SynVox", "SynVox 2", "St.SynVox", "Phase SynVox"]),
    (55, 1, ["Ana Voices", "Ana Voices 2", "Ana Voices 3", "Lead Ana.Vox"]),
    (56, 0, ["Orchestrahit", "Orc Hit 2", "St.Orc Hit", "Enh.Orc Hit"]),
    (56, 1, ["Bass Hit", "Bass Hit 2", "St.Bass Hit", "PhaseBassHit"]),
    (56, 2, ["6th Hit", "6th Hit 2", "St.6th Hit", "Dly.6th Hit"]),
    (56, 3, ["Euro Hit", "Euro Hit 2", "St.Euro Hit", "Dly.Euro Hit"]),
    // ---- Brass ----
    (57, 0, ["Trumpet", "Solo Trumpet", "Romantic Tp", "Enh.Trumpet"]),
    (57, 1, ["Dark Trumpet", "Mild Trumpet", "Tp.Dark vib", "Warm Trumpet"]),
    (58, 0, ["Trombone", "Solo Bone", "Trombone vib", "Enh.Trombone"]),
    (58, 1, ["Trombone 2", "Solo Bone 2", "Trombone2vib", "Enh.Bone 2"]),
    (58, 2, ["Brite Bone", "Brite Bone 2", "Br.Bone vib", "Enh.Br Bone"]),
    (59, 0, ["Tuba", "Tuba 2", "Tuba vib", "Chorus Tuba"]),
    (60, 0, ["MuteTrumpet", "MuteTrumpet2", "Solo MutedTp", "Enh.Muted Tp"]),
    (60, 1, ["MuteTrumpet2", "Harmon Mute", "Harmon Mute2", "Enh.MutedTp2"]),
    (61, 0, ["French Horns", "FrenchHorns2", "St.Fr Horns", "Enh.StFrHorn"]),
    (61, 1, ["Fr.Horn", "MildFr.Horns", "St.Fr Horns2", "Warm Horns"]),
    (62, 0, ["Brass 1", "Brass FF", "St.Brass", "St.Big Brass"]),
    (62, 1, ["Brass 2", "BrassSection", "St.Brass 2", "Enh.Brs Sect"]),
    (63, 0, ["SynthBrass 1", "JP Syn.Brass", "Hyper Brass", "SuperJP Brs1"]),
    (63, 1, ["SynthBrass 3", "JPSyn.Brass2", "Stack Brass", "Lead Brass"]),
    (63, 2, ["Oct.SynBrass", "OctSynBrass2", "OctSynBrass3", "Phase OctBrs"]),
    (63, 3, ["Jump Brass", "80's Brass", "SuperSaw Brs", "SuperJP Brs2"]),
    (64, 0, ["SynthBrass 2", "MG Syn.Horn", "Warm SynHorn", "WarmSynHorn2"]),
    (64, 1, ["SynthBrass 4", "OB Syn.Horn", "Rich SynHorn", "Phase Horn"]),
    (64, 2, ["Velo Brass", "CS Syn.Brass", "P5 Syn.Brass", "Fat Pro Bras"]),
    // ---- Reed ----
    (65, 0, ["Soprano Sax", "Soprano Sax2", "Sop.Sax vib", "Enh.Sop Sax"]),
    (66, 0, ["Alto Sax", "Breathy Alto", "AltoSoft vib", "Enh.Alto Sax"]),
    (67, 0, ["Tenor Sax", "BreathyTenor", "Blow Tenor", "Enh.TenorSax"]),
    (68, 0, ["Bariton Sax", "Barely Bari", "Bari.Sax vib", "Enh.Bari Sax"]),
    (69, 0, ["Oboe", "Brite Oboe", "Classic Oboe", "Enh.Oboe"]),
    (70, 0, ["EnglishHorn", "EnglishHorn2", "E.Horn vib", "Enh.E Horn"]),
    (71, 0, ["Bassoon", "Bassoon 2", "Bassoon vib", "Enh.Bassoon"]),
    (72, 0, ["Clarinet", "Br.Clarinet", "JazzClarinet", "Jz.Clarinet2"]),
    // ---- Pipe ----
    (73, 0, ["Piccolo", "Piccolo 2", "Piccolo vib", "Enh.Piccolo"]),
    (74, 0, ["Flute", "Flute 2", "Flute vib", "Enh.Flute"]),
    (75, 0, ["Recorder", "Recorder 2", "Recorder vib", "Enh.Recorder"]),
    (76, 0, ["Pan Flute", "Pan Flute 2", "PanFlute vib", "Cho.PanFlute"]),
    (77, 0, ["Bottle Blow", "Bottle Blow2", "Bottle vib", "Phase Bottle"]),
    (78, 0, ["Shakuhachi", "Shakuhachi 2", "Shaku.vib", "Delay Shaku"]),
    (79, 0, ["Whistle", "Whistle 2", "Whistle vib", "DelayWhistle"]),
    (80, 0, ["Ocarina", "Ocarina 2", "Ocarina vib", "DelayOcarina"]),
    // ---- Synth lead ----
    (81, 0, ["Square Wave", "MG Square", "OB Square", "OBSquareLead"]),
    (81, 1, ["Square", "Fat Square", "Fat Square2", "Phase Square"]),
    (81, 2, ["Sine Wave", "2600 Sine", "2600 Sine 2", "Sine Lead"]),
    (82, 0, ["Saw Wave", "JP Saw Wave", "Oct.JP Saw", "KeySync Saw"]),
    (82, 1, ["Saw", "MG Saw", "Hybrid Saw", "Flanging Saw"]),
    (82, 2, ["Doctor Solo", "Fat Saw Solo", "Hybrid Solo", "Doctor Lead"]),
    (82, 3, ["Natural Lead", "P5 Saw Lead", "MG Saw Lead", "Fat Saw Lead"]),
    (82, 4, ["SequencedSaw", "MG Sequence", "DelaySeqence", "PhaseSeqence"]),
    (83, 0, ["Syn.Calliope", "SynCalliope2", "SynCalliope3", "LeadCalliope"]),
    (84, 0, ["Chiffer Lead", "ChifferLead2", "ChifferLead3", "Chiffers"]),
    (85, 0, ["Charang", "Charang 2", "Charang 3", "Charang Lead"]),
    (85, 1, ["Wire Lead", "Wire Lead 2", "Wire Lead 3", "Phase Wire"]),
    (86, 0, ["Solo Vox", "Solo Vox 2", "Solo Vox 3", "SoloVox Lead"]),
    (87, 0, ["5th SawWave", "5th SawWave2", "5th SawWave3", "Flanging 5th"]),
    (88, 0, ["Bass & Lead", "Bass & Lead2", "Bass & Lead3", "Phase BsLead"]),
    (88, 1, ["DelayedLead", "DelayedLead2", "DelayedLead3", "Suffle Lead"]),
    // ---- Synth pad, etc ----
    (89, 0, ["Fantasia", "Fantasia 2", "Fantasia 3", "New Fantasia"]),
    (90, 0, ["Warm Pad", "Warm Pad 2", "Warm Pad 3", "Phase Pad"]),
    (90, 1, ["Sine Pad", "Sine Pad 2", "Sine Pad 3", "Chorus Sine"]),
    (91, 0, ["Polysynth", "Polysynth 2", "Polysynth 3", "KeySyncSynth"]),
    (92, 0, ["SpaceVoices", "SpaceVoices 2", "SpaceVoices 3", "Phase Voices"]),
    (92, 1, ["Itopia", "Itopia 2", "Itopia 3", "Pan Itopia"]),
    (93, 0, ["BowedGlass", "BowedGlass 2", "BowedGlass 3", "Ring Glass"]),
    (94, 0, ["Metal Pad", "Metal Pad 2", "Metal Pad 3", "Space Pad"]),
    (95, 0, ["Halo Pad", "Halo Pad 2", "Halo Pad 3", "Phase Halo"]),
    (96, 0, ["Sweep Pad", "Sweep Pad 2", "Sweep Pad 3", "Flanging Pad"]),
    // ---- Synth SFX ----
    (97, 0, ["Ice Rain", "Ice Rain 2", "Ice Rain 3", "Reverse Rain"]),
    (98, 0, ["Soundtrack", "Soundtrack 2", "Soundtrack 3", "Phase Track"]),
    (99, 0, ["Crystal", "Crystal 2", "Crystal 3", "3D Crystal"]),
    (99, 1, ["Syn Mallet", "Syn Mallet 2", "Syn Mallet 3", "Phase Mallet"]),
    (100, 0, ["Atmosphere", "Atmosphere 2", "Atmosphere 3", "Pan Atmos"]),
    (101, 0, ["Brightness", "Brightness 2", "Brightness 3", "Bright Star"]),
    (102, 0, ["Goblin", "Goblin 2", "Goblin 3", "Rev Goblin"]),
    (103, 0, ["Echo Drops", "Echo Drops 2", "Echo Drops 3", "Delay Drops"]),
    (103, 1, ["Echo Bell", "Echo Bell 2", "Echo Bell 3", "Delay Bell"]),
    (103, 2, ["Echo Pan", "Echo Pan 2", "Echo Pan 3", "Delay Pan"]),
    (104, 0, ["Star Theme", "Star Theme 2", "Star Theme 3", "Phase Theme"]),
    // ---- Ethnic, etc ----
    (105, 0, ["Sitar", "Atk Sitar", "St.Sitar", "Enh.Sitar"]),
    (105, 1, ["Sitar 2", "Atk Sitar 2", "St.Sitar 2", "FantasySitar"]),
    (106, 0, ["Banjo", "Banjo 2", "St.Banjo", "St.Banjo 2"]),
    (107, 0, ["Shamisen", "Shamisen 2", "St.Shamisen", "St.Shamisen2"]),
    (108, 0, ["Koto", "Koto 2", "St.Koto", "St.Koto 2"]),
    (108, 1, ["Taisho Koto", "Taisho Koto2", "St.T Koto", "St.T Koto 2"]),
    (109, 0, ["Kalimba", "Kalimba 2", "St.Kalimba", "Trem.Kalimba"]),
    (110, 0, ["Bag Pipe", "Bag Pipe 2", "St.Bag Pipe", "Enh.Bag Pipe"]),
    (111, 0, ["Fiddle", "Fiddle vib", "Fiddle 2 vib", "Enh.Fiddle"]),
    (112, 0, ["Shanai", "Shanai 2", "St.Shanai", "Enh.Shanai"]),
    // ---- Percussive (Classical のみ) ----
    (113, 0, ["Tinkle Bell", "", "", ""]),
    (114, 0, ["Agogo", "", "", ""]),
    (115, 0, ["Steel Drums", "", "", ""]),
    (116, 0, ["Woodblock", "", "", ""]),
    (116, 1, ["Castanet", "", "", ""]),
    (117, 0, ["Taiko", "", "", ""]),
    (117, 1, ["Concert BD", "", "", ""]),
    (118, 0, ["Melo.Tom 1", "", "", ""]),
    (118, 1, ["Melo.Tom 2", "", "", ""]),
    (119, 0, ["Synth Drum", "", "", ""]),
    (119, 1, ["808 tom", "", "", ""]),
    (119, 2, ["Elec Perc", "", "", ""]),
    (120, 0, ["Reverse Cym", "", "", ""]),
    // ---- SFX (Classical のみ) ----
    (121, 0, ["GtFret Noise", "", "", ""]),
    (121, 1, ["Gt.Cut Noise", "", "", ""]),
    (121, 2, ["Slap_St.Bass", "", "", ""]),
    (122, 0, ["Breath Noise", "", "", ""]),
    (122, 1, ["Fl.Key Click", "", "", ""]),
    (123, 0, ["Seashore", "", "", ""]),
    (123, 1, ["Rain", "", "", ""]),
    (123, 2, ["Thunder", "", "", ""]),
    (123, 3, ["Wind", "", "", ""]),
    (123, 4, ["Stream", "", "", ""]),
    (123, 5, ["Bubble", "", "", ""]),
    (124, 0, ["Bird Tweet", "", "", ""]),
    (124, 1, ["Dog", "", "", ""]),
    (124, 2, ["Horse Gallop", "", "", ""]),
    (124, 3, ["Bird Tweet 2", "", "", ""]),
    (125, 0, ["Telephone", "", "", ""]),
    (125, 1, ["Telephone 2", "", "", ""]),
    (125, 2, ["Door Creak", "", "", ""]),
    (125, 3, ["Door", "", "", ""]),
    (125, 4, ["Scratch", "", "", ""]),
    (125, 5, ["Wind Chimes", "", "", ""]),
    (126, 0, ["Helicopter", "", "", ""]),
    (126, 1, ["Car-Engine", "", "", ""]),
    (126, 2, ["Car-Stop", "", "", ""]),
    (126, 3, ["Car-Pass", "", "", ""]),
    (126, 4, ["Car-Crash", "", "", ""]),
    (126, 5, ["Siren", "", "", ""]),
    (126, 6, ["Train", "", "", ""]),
    (126, 7, ["Jetplane", "", "", ""]),
    (126, 8, ["Starship", "", "", ""]),
    (126, 9, ["Burst Noise", "", "", ""]),
    (127, 0, ["Applause", "", "", ""]),
    (127, 1, ["Laughing", "", "", ""]),
    (127, 2, ["Screaming", "", "", ""]),
    (127, 3, ["Punch", "", "", ""]),
    (127, 4, ["Heart Beat", "", "", ""]),
    (127, 5, ["Footsteps", "", "", ""]),
    (128, 0, ["Gunshot", "", "", ""]),
    (128, 1, ["Machine Gun", "", "", ""]),
    (128, 2, ["Lasergun", "", "", ""]),
    (128, 3, ["Explosion", "", "", ""]),
];

/// Special 1 Set (MSB 80) / Special 2 Set (MSB 81)。どちらも LSB 0、PC 1-128 (p.134)。
pub const SPECIAL: [[&str; 2]; 128] = [
    ["D.L.A.Pad", "Blown Bass"],
    ["BrushingSaw", "Enh.Violin"],
    ["Xtremities", "Solo Violin"],
    ["Atmostrings", "Enh.Cello"],
    ["Noo Tongs", "Overture Str"],
    ["Mistery", "St.Trem Str2"],
    ["EastrnEurope", "St.Strings 2"],
    ["Harpsi&Str", "Intim8String"],
    ["Shout Gt", "Str&Brs Orch"],
    ["Clean Chorus", "St.Orchestr2"],
    ["Mid Boost Gt", "St.Slow Str2"],
    ["Guitarvibe", "ChamberPlyrs"],
    ["Cluster Sect", "Prelude"],
    ["Mariachi Tp", "Tape Strings"],
    ["NY Tenor", "JP Strings"],
    ["Jazz Club", "PhaseSyn.Str"],
    ["Moody Alto", "SciFi Choir"],
    ["Fuji Yama", "Rich Choir"],
    ["SD Piano", "St.Sm Choir2"],
    ["Piano&Str", "Tron Choir"],
    ["WarmVoxPiano", "ScatterDanze"],
    ["Enh.E Grand", "Choral Brass"],
    ["Brite Honky", "Vocal Keys"],
    ["Ambient EP", "SampleThe80s"],
    ["Phase Dyno", "Spectravox"],
    ["Wah Dyno", "Formantic"],
    ["Voweling EP", "Vocoder Wave"],
    ["Stage 73", "Enh.Muted Tp"],
    ["NY Rhodes", "St.Big Brass"],
    ["Amped Wurlie", "ConcertHorns"],
    ["Dirty Wurlie", "Brass Orch"],
    ["PhaseFlangEP", "Loose Lips"],
    ["Dist Wurly", "Multi Brass"],
    ["Xmod EP", "SuperJP Brs1"],
    ["EP+Followers", "SuperJP Brs2"],
    ["Backrhodes", "DragginBrass"],
    ["FM Delight", "Enh.Alto Sax"],
    ["Enh.Legend", "Enh.TenorSax"],
    ["Phasing EP", "Enh.Bari Sax"],
    ["Comp Clav", "Sax's Sect"],
    ["Vibarimba", "Reed Romance"],
    ["FM layer", "U.S.Patrol"],
    ["Vibey Flute", "Enh.Oboe"],
    ["Chime Bells", "Jz.Clarinet2"],
    ["The Big Spin", "Enh.Flute"],
    ["Roller Spin", "Tron Flutes"],
    ["Rocker Spin", "Flute&Clari"],
    ["Tone Wh.Solo", "Orch Reeds"],
    ["Purple Spin", "Delay Shaku"],
    ["RingingOrgan", "DelayOcarina"],
    ["60's LeadOrg", "DynOrchestra"],
    ["DistLead Org", "Celtic Ens"],
    ["Assalt Organ", "Early Ens"],
    ["Perky Spin", "WoodSymphony"],
    ["Gospel Spin", "ViennaWoods2"],
    ["Boogie Organ", "Stage Woods"],
    ["Klubb Organ", "TremoloFlute"],
    ["96 Year", "SupportWoods"],
    ["OrganInStone", "Fiddle&Oboe"],
    ["StringsOrgan", "Mono Blues"],
    ["Cathdr Pipes", "TravlnFlutes"],
    ["Enh.Theater", "DistordedSAW"],
    ["Ld.Harmonica", "The Leader"],
    ["Guitar&Str", "SH-2 Lead"],
    ["Elec Gut Gt", "JP-6 Lead"],
    ["Chorus GutGt", "Glassy Saws"],
    ["Enh.Nylon Gt", "Too Pure"],
    ["CompSteel Gt", "Mono Analog"],
    ["Elec Aco.Gt", "Charang Lead"],
    ["Hard Steel", "Phase Wire"],
    ["Comp OVSteel", "Flanging 5th"],
    ["DelayedSteel", "BOG"],
    ["Wah Steel Gt", "Solo Twang"],
    ["Soft Steel", "Koto Power"],
    ["Lead Jazz Gt", "Dulcid Solo"],
    ["Hawaian Gt", "Wine Guitar"],
    ["Solo Jazz Gt", "Leapin' Keys"],
    ["Old Clean Gt", "New Fantasia"],
    ["Jazz Chorus", "MilleniumStr"],
    ["Fat Clean", "OB Borealis"],
    ["Space-D Gt", "Wet Glass"],
    ["Whammy Gt", "VintagePhase"],
    ["Trem&Cho Gt", "P-layer"],
    ["Long DLY.Gt", "Ring Glass"],
    ["Comp FeedBK", "5th Atm /Aft"],
    ["ChaseDL Mute", "Combing"],
    ["Wah Funk Pop", "Modular Life"],
    ["Old Blues", "KeySync Saw"],
    ["335 Drive", "Phase Track"],
    ["335 Drive2", "3D Crystal"],
    ["SmallAMP OD", "Bright Star"],
    ["CountrySlide", "Technoheadz"],
    ["335 Comp", "Techno Cave"],
    ["TC Lead Gt", "Wedo-Wodo"],
    ["Lead Mild Gt", "Dance Sweet"],
    ["Crunch Gt", "Dancefloor"],
    ["Auto Wah Gt", "Frogcillator"],
    ["Folmant Gt", "Dance Zipper"],
    ["Fuzz Gt", "Lo-fi Chord"],
    ["MonoPhase Gt", "Sliced"],
    ["2 Layer Gt", "Dance Chop"],
    ["MonoDLY Dist", "GO WILD!"],
    ["Step FLG Gt", "Tropics"],
    ["335 Fuzz", "FM Fx Bells"],
    ["Drive Funk", "CrystalGlass"],
    ["Loud & Metal", "Borealis"],
    ["Heavy DistGt", "Circular Pad"],
    ["Power Mute", "Oxigenizer"],
    ["Muted OD", "Quasar"],
    ["Combo Drive", "PsycheSweep"],
    ["Ring Gt.", "Hell Section"],
    ["Scream Drive", "DigitalDrone"],
    ["Gt.Pinch Wah", "Harp Drop"],
    ["Power DistGt", "Runaway Rez"],
    ["Triple Gt", "Wacky Pizzer"],
    ["Reverse Harm", "Reverse Rain"],
    ["Drivin'Uprit", "Starburst"],
    ["Enh.Aco Bass", "Tongan Delay"],
    ["Pre Bass", "Cascade"],
    ["Comp Finger", "Legat->Stacat"],
    ["Octrv Finger", "Rev Goblin"],
    ["Rock Bass", "FantasySitar"],
    ["Dist Bass", "Rising Sun"],
    ["Solo Fretles", "Green Tea"],
    ["WahSlapBass", "Jimmy's Koto"],
    ["Froggy Bass", "Fly 2 India"],
    ["3D TB-303", "IndianSpirit"],
    ["Acid Dist Bs", "Gt/BsNz MENU"],
];

/// ドラムセット (PC 1-128, [Classical, Contemporary, Solo, Enhanced]) (p.139)。
/// マニュアルの "<-" (左と同じ) は展開して持つ。
pub const DRUMS: &[(u8, [&str; 4])] = &[
    (1, ["Standard Set", "StandardSet2", "St.Standard", "Amb.Standard"]),
    (9, ["Room Set", "Room Set 2", "St.Room", "Amb.Room"]),
    (17, ["Power Set", "Power Set 2", "St.Power", "Gated Power"]),
    (25, ["Electric Set", "Dance Set", "Rust Set", "Techno Set"]),
    (26, ["Analog Set", "Rave Set", "Analog2 Set", "Bully Set"]),
    (33, ["Jazz Set", "Jazz Set 2", "St.Jazz", "Amb.Jazz"]),
    (41, ["Brush Set", "Brush Set 2", "St.Brush", "Amb.Brush"]),
    (49, ["OrchestraSet", "OrchestraSet", "OrchestraSet", "OrchestraSet"]),
    (57, ["SFX Set", "SFX Set", "SFX Set", "SFX Set"]),
];

/// MFX のタイプ名 (Parameter List / Owner's Manual p.105-)。0 は THROUGH。
pub const MFX_TYPES: [&str; 91] = [
    "THROUGH",
    "STEREO EQ",
    "OVERDRIVE",
    "DISTORTION",
    "PHASER",
    "SPECTRUM",
    "ENHANCER",
    "AUTO WAH",
    "ROTARY",
    "COMPRESSOR",
    "LIMITER",
    "HEXA-CHORUS",
    "TREMOLO CHORUS",
    "SPACE-D",
    "STEREO CHORUS",
    "STEREO FLANGER",
    "STEP FLANGER",
    "STEREO DELAY",
    "MODULATION DELAY",
    "TRIPLE TAP DELAY",
    "QUADRUPLE TAP DELAY",
    "TIME CONTROL DELAY",
    "2VOICE PITCH SHIFTER",
    "FBK PITCH SHIFTER",
    "REVERB",
    "GATED REVERB",
    "OVERDRIVE -> CHORUS",
    "OVERDRIVE -> FLANGER",
    "OVERDRIVE -> DELAY",
    "DISTORTION -> CHORUS",
    "DISTORTION -> FLANGER",
    "DISTORTION -> DELAY",
    "ENHANCER -> CHORUS",
    "ENHANCER -> FLANGER",
    "ENHANCER -> DELAY",
    "CHORUS -> DELAY",
    "FLANGER -> DELAY",
    "CHORUS -> FLANGER",
    "CHORUS/DELAY",
    "FLANGER/DELAY",
    "CHORUS/FLANGER",
    "STEREO PHASER",
    "KEYSYNC FLANGER",
    "FORMANT FILTER",
    "RING MODULATOR",
    "MULTI TAP DELAY",
    "REVERSE DELAY",
    "SHUFFLE DELAY",
    "3D DELAY",
    "3VOICE PITCH SHIFTER",
    "LOFI COMPRESS",
    "LOFI NOISE",
    "SPEAKER SIMULATOR",
    "OVERDRIVE 2",
    "DISTORTION 2",
    "STEREO COMPRESSOR",
    "STEREO LIMITER",
    "GATE",
    "SLICER",
    "ISOLATOR",
    "3D CHORUS",
    "3D FLANGER",
    "TREMOLO",
    "AUTO PAN",
    "STEREO PHASER 2",
    "STEREO AUTO WAH",
    "ST FORMANT FILTER",
    "MULTI TAP DELAY 2",
    "REVERSE DELAY 2",
    "SHUFFLE DELAY 2",
    "3D DELAY 2",
    "ROTARY 2",
    "ROTARY MULTI",
    "KEYBOARD MULTI",
    "RHODES MULTI",
    "JD MULTI",
    "STEREO LOFI COMPRESS",
    "STEREO LOFI NOISE",
    "GUITAR AMP SIMULATOR",
    "STEREO OVERDRIVE",
    "STEREO DISTORTION",
    "GUITAR MULTI A",
    "GUITAR MULTI B",
    "GUITAR MULTI C",
    "CLEAN GUITAR MULTI A",
    "CLEAN GUITAR MULTI B",
    "BASS MULTI",
    "ISOLATOR 2",
    "STEREO SPECTRUM",
    "3D AUTO SPIN",
    "3D MANUAL",
];

/// (セット, PC, LSB) の楽器名。表に無ければ `None`。
pub fn inst_name(set: SoundSet, pc: u8, lsb: u8) -> Option<&'static str> {
    match set {
        SoundSet::Special1 | SoundSet::Special2 => {
            if lsb != 0 || !(1..=128).contains(&pc) {
                return None;
            }
            Some(SPECIAL[pc as usize - 1][(set == SoundSet::Special2) as usize])
        }
        _ => {
            let i = set.gm2_index()?;
            INST.iter()
                .find(|(p, l, _)| *p == pc && *l == lsb)
                .map(|(_, _, n)| n[i])
                .filter(|n| !n.is_empty())
        }
    }
}

/// このセット・PC で選べる Variation (LSB) の一覧。
pub fn variations(set: SoundSet, pc: u8) -> impl Iterator<Item = (u8, &'static str)> {
    let special = set.is_special();
    let idx = set.gm2_index();
    let sp = if special && (1..=128).contains(&pc) {
        Some((0u8, SPECIAL[pc as usize - 1][(set == SoundSet::Special2) as usize]))
    } else {
        None
    };
    INST.iter()
        .filter(move |(p, _, n)| !special && *p == pc && idx.is_some_and(|i| !n[i].is_empty()))
        .map(move |(_, l, n)| (*l, n[idx.unwrap_or(0)]))
        .chain(sp)
}

/// このセットの capital sound (Variation 0) の一覧。`(PC, 名前)`。
pub fn capitals(set: SoundSet) -> Vec<(u8, &'static str)> {
    (1..=128u8)
        .filter_map(|pc| inst_name(set, pc, 0).map(|n| (pc, n)))
        .collect()
}

/// ドラムセット名。
pub fn drum_name(set: SoundSet, pc: u8) -> Option<&'static str> {
    let i = set.gm2_index()?;
    DRUMS.iter().find(|(p, _)| *p == pc).map(|(_, n)| n[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_complete() {
        // 各 PC に Variation 0 があり、LSB は 0 から詰まっている。
        let mut prev = (0u8, 0u8);
        for &(pc, lsb, _) in INST {
            if pc != prev.0 {
                assert_eq!(pc, prev.0 + 1, "PC {pc} が飛んでいる");
                assert_eq!(lsb, 0, "PC {pc} が Variation 0 から始まらない");
            } else {
                assert_eq!(lsb, prev.1 + 1, "PC {pc} の LSB が飛んでいる");
            }
            prev = (pc, lsb);
        }
        assert_eq!(prev.0, 128);
        for &set in &SoundSet::ALL {
            assert!(set.is_special() || capitals(set).len() == 112 || set == SoundSet::Classical);
        }
        assert_eq!(capitals(SoundSet::Classical).len(), 128);
    }

    #[test]
    fn lookups() {
        assert_eq!(inst_name(SoundSet::Contemporary, 1, 0), Some("Ac.Piano"));
        assert_eq!(inst_name(SoundSet::Enhanced, 128, 0), None);
        assert_eq!(inst_name(SoundSet::Special2, 128, 0), Some("Gt/BsNz MENU"));
        assert_eq!(drum_name(SoundSet::Solo, 25), Some("Rust Set"));
        assert_eq!(variations(SoundSet::Classical, 126).count(), 10);
        assert_eq!(SoundSet::from_native_msb(105), Some((SoundSet::Contemporary, true)));
    }
}
