//! Effect types: the per-slot state persisted in a project, the parameter
//! sets the DSP nodes consume, and the descriptor tables that give every
//! parameter one authoritative range and normalization.
//!
//! Effects are chainable units that run after a channel's generator; see
//! `docs/archive/EFFECTS_PLAN.md` for the plumbing and
//! `docs/MODULATION.md` for why descriptors exist and what the parameter
//! model is going to become.

/// Effect kind. The tag for [`EffectParams`], mirroring how `ChannelSource`
/// tags its per-source state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Eq,
    Modulation,
    Filter,
    Drive,
    Bitcrush,
    Delay,
    Reverb,
    Plate,
    Gate,
    Compressor,
    Limiter,
    Buffer,
    /// The channel strip's input stage, insertable on its own.
    ///
    /// Serialized as `preamp`, which is what `rename_all = "snake_case"`
    /// would give it anyway — stated rather than inherited, because the
    /// ML-M1 shipped under a name nobody chose and that is frozen forever.
    #[serde(rename = "preamp")]
    Preamp,
    /// A container: a device that holds an ordered run of the devices after
    /// it. See `docs/plans/containers/02-the-container-is-a-device.md`.
    Chain,
    /// A container whose direct children are **parallel branches**, summed.
    ///
    /// The fifteenth kind and the second that is not an effect. A branch is a
    /// direct child's *run*: a leaf child is a one-device branch, and a
    /// `Chain` child is a branch holding as many devices as it likes, which
    /// is how Drive → Delay becomes one branch with no new syntax.
    ///
    /// Each branch starts from the layer's input, the shorter ones are held
    /// back to meet the longest, and the branches sum at unity before the
    /// layer's mix blends the result against its dry copy
    /// (`docs/plans/containers/08-the-chain-splits-and-sums.md`). A layer of
    /// one branch is a chain, sample for sample.
    Layer,
}

/// How a container runs the rows it holds. See [`EffectKind::container_flow`].
///
/// Both kinds hold the same thing -- `ContainerParams`, a child count and a
/// mix -- and store it the same way. What differs is what the renderer does
/// with the span, and what the latency walk and the rack's drawing say about
/// it, which are the three readers `docs/plans/containers/07` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerFlow {
    /// Each row into the next, in rack order: a `Chain`. The rows' latencies
    /// add.
    Series,
    /// The input split across the **direct children**, each child's run a
    /// branch, and the branches summed: a `Layer`. The longest branch is the
    /// latency, and the others are delayed to meet it.
    Parallel,
}

/// Base-rate frames of latency the 2x oversampler's interpolate/process/
/// decimate path adds.
///
/// Declared here rather than in `mooloop-dsp` beside the oversampler because
/// it is an *interface* number now: the control thread sizes a compensation
/// delay from it before any node exists to ask
/// (`docs/plans/latency-compensation/02-declared-latency.md`). The DSP reads
/// it from here, so there is one figure rather than two that must agree.
pub const OVERSAMPLER_LATENCY_FRAMES: u32 = 15;

/// Base-rate frames the Limiter holds its audio back by, so its gain
/// computer sees every peak before it has to be let out (MOO-142).
///
/// An interface number for the same reason as
/// [`OVERSAMPLER_LATENCY_FRAMES`]: the compensation plan is sized from it
/// before any node exists. Frames rather than milliseconds, because a
/// declared latency cannot depend on the rate: 2 ms at 48 kHz, 2.2 ms at
/// 44.1 kHz, 1 ms at 96 kHz. The lookahead's length sets how fast the
/// attack is, not whether the ceiling holds.
pub const LIMITER_LATENCY_FRAMES: u32 = 96;

impl EffectKind {
    /// Base-rate frames this kind adds to the signal passing through it.
    ///
    /// A *declaration*, answerable without building a node, which is what lets
    /// the graph plan be compiled on the control thread where its delays can
    /// be allocated. `AudioNode::latency_frames` is the same number from the
    /// running node, and `mooloop-dsp` asserts the two agree per kind — two
    /// numbers for one fact is how they come to disagree, and a disagreement
    /// here would size a compensation delay wrongly and move the misalignment
    /// rather than remove it.
    ///
    /// Zero is the common and correct answer: an effect that processes sample
    /// by sample adds no latency, and most of them do.
    pub fn latency_frames(self) -> u32 {
        match self {
            // The only device with an internal oversampled path today.
            Self::Drive => OVERSAMPLER_LATENCY_FRAMES,
            // Lookahead (MOO-142).
            Self::Limiter => LIMITER_LATENCY_FRAMES,
            Self::Eq
            | Self::Modulation
            | Self::Filter
            // Sample by sample, filters and all: the tilt pair is a biquad
            // sandwich, not a lookahead.
            | Self::Preamp
            | Self::Bitcrush
            | Self::Delay
            | Self::Reverb
            | Self::Plate
            | Self::Gate
            | Self::Compressor
            | Self::Buffer
            // A container declares nothing of its own. Its children are rows
            // of the same chain, so `chain_latency` already counts them; a
            // sum here would count them twice. See question 4 in
            // `docs/plans/containers/README.md`.
            | Self::Chain
            | Self::Layer => 0,
        }
    }

    /// Whether this kind holds a run of the rows after it.
    ///
    /// **The one place a kind is asked whether it is a container.** It was
    /// spelled `== EffectKind::Chain` at each of its call sites, which is the
    /// same shape as the params-level test below and drifts for the same
    /// reason: a second container kind was arriving
    /// (`docs/plans/containers/07-a-branch-is-a-run.md`) and every site that
    /// named `Chain` by hand was a site that would keep meaning "a serial
    /// container" when it meant "a container".
    ///
    /// Read off [`Self::container_flow`] rather than matched a second time,
    /// so "is a container" and "how does it run" cannot disagree about which
    /// kinds are containers: there is one match, and this is its `is_some`.
    ///
    /// `EffectParams::is_container` is this question asked of a value rather
    /// than of a kind, and `container_predicates_agree_about_every_kind`
    /// holds the two together.
    pub fn is_container(self) -> bool {
        self.container_flow().is_some()
    }

    /// How a container runs the rows it holds, or `None` for a device that
    /// holds nothing.
    ///
    /// The one question two container kinds answer differently, and so the
    /// one place the words `Chain` and `Layer` are told apart. Everything
    /// about *where* a container's rows are -- its span, its depth, whether
    /// an index falls inside it -- is the same for both and is asked of
    /// [`EffectParams::container_children`] instead; nothing in
    /// `structure.rs` reads this.
    ///
    /// Introduced with `Layer` rather than before it, deliberately: with one
    /// container kind in the tree, `Option<ContainerFlow>` would have had one
    /// variant and been `is_container()` spelled a second way.
    pub fn container_flow(self) -> Option<ContainerFlow> {
        match self {
            Self::Chain => Some(ContainerFlow::Series),
            Self::Layer => Some(ContainerFlow::Parallel),
            Self::Eq
            | Self::Modulation
            | Self::Filter
            | Self::Drive
            | Self::Preamp
            | Self::Bitcrush
            | Self::Delay
            | Self::Reverb
            | Self::Plate
            | Self::Gate
            | Self::Compressor
            | Self::Limiter
            | Self::Buffer => None,
        }
    }

    /// Every kind, in the order the UI offers them when adding an effect.
    pub const ALL: [EffectKind; 15] = [
        EffectKind::Eq,
        EffectKind::Modulation,
        EffectKind::Filter,
        EffectKind::Preamp,
        EffectKind::Drive,
        EffectKind::Bitcrush,
        EffectKind::Delay,
        EffectKind::Reverb,
        EffectKind::Plate,
        EffectKind::Gate,
        EffectKind::Compressor,
        EffectKind::Limiter,
        EffectKind::Buffer,
        EffectKind::Chain,
        EffectKind::Layer,
    ];

    /// Display name for device headers and the add-effect picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::Eq => "EQ",
            Self::Modulation => "Mod",
            Self::Filter => "Filter",
            Self::Preamp => "Preamp",
            Self::Drive => "Drive",
            Self::Bitcrush => "Bitcrush",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Plate => "Plate",
            Self::Gate => "Gate",
            Self::Compressor => "Comp",
            Self::Limiter => "Limiter",
            Self::Buffer => "Buffer",
            Self::Chain => "Chain",
            Self::Layer => "Layer",
        }
    }

    /// This kind's parameter table. Indexed by position, not by `id` — read
    /// [`ParamDescriptor::id`] for the value that goes on the wire.
    pub fn descriptors(self) -> &'static [ParamDescriptor] {
        match self {
            Self::Eq => &EQ_DESCRIPTORS,
            Self::Modulation => &MODULATION_DESCRIPTORS,
            Self::Filter => &FILTER_DESCRIPTORS,
            Self::Preamp => &PREAMP_DESCRIPTORS,
            Self::Drive => &DRIVE_DESCRIPTORS,
            Self::Bitcrush => &BITCRUSH_DESCRIPTORS,
            Self::Delay => &DELAY_DESCRIPTORS,
            Self::Reverb => &REVERB_DESCRIPTORS,
            Self::Plate => &PLATE_DESCRIPTORS,
            Self::Gate => &GATE_DESCRIPTORS,
            Self::Compressor => &COMPRESSOR_DESCRIPTORS,
            Self::Limiter => &LIMITER_DESCRIPTORS,
            Self::Buffer => &BUFFER_DESCRIPTORS,
            Self::Chain | Self::Layer => &CONTAINER_DESCRIPTORS,
        }
    }

    /// Look one parameter up by its wire id.
    pub fn descriptor(self, id: u32) -> Option<&'static ParamDescriptor> {
        self.descriptors().iter().find(|d| d.id == id)
    }

    /// Default parameter set for a freshly added effect of this kind.
    pub fn default_params(self) -> EffectParams {
        match self {
            Self::Eq => EffectParams::Eq(EqParams::default()),
            Self::Modulation => EffectParams::Modulation(ModulationParams::default()),
            Self::Filter => EffectParams::Filter(FilterParams::default()),
            Self::Preamp => EffectParams::Preamp(PreampParams::default()),
            Self::Drive => EffectParams::Drive(DriveParams::default()),
            Self::Bitcrush => EffectParams::Bitcrush(BitcrushParams::default()),
            Self::Delay => EffectParams::Delay(DelayParams::default()),
            Self::Reverb => EffectParams::Reverb(ReverbParams::default()),
            Self::Plate => EffectParams::Plate(PlateParams::default()),
            Self::Gate => EffectParams::Gate(GateParams::default()),
            Self::Compressor => EffectParams::Compressor(CompressorParams::default()),
            Self::Limiter => EffectParams::Limiter(LimiterParams::default()),
            Self::Buffer => EffectParams::Buffer(BufferParams::default()),
            Self::Chain => EffectParams::Chain(ContainerParams::default()),
            Self::Layer => EffectParams::Layer(ContainerParams::default()),
        }
    }
}

/// How a parameter's normalized 0..1 knob position maps onto its natural
/// range. Events on the wire always carry natural units; this is the mapping
/// the non-realtime side applies before sending them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamCurve {
    /// Even across the range. Mixes, depths, bipolar tilts.
    Linear,
    /// Even in ratio, so a knob feels right on frequencies and gains.
    /// Requires `min > 0`.
    Exponential,
    /// `n` discrete positions mapped across the range: mode selectors.
    ///
    /// Wider than a `u8` because a selector may name a channel, and a
    /// project may hold [`crate::MAX_CHANNELS`] of them plus the "none"
    /// position -- 257 positions, which is two more than a byte of *count*
    /// can express. The width was never a design decision; it was the
    /// default integer, and the first selector that needed more than 255
    /// positions is the one that says so.
    Stepped(u16),
    /// The mixer fader's taper: normalized is fader travel, read through
    /// [`crate::gain::FADER_BREAKPOINTS`], and natural is linear gain. Unity
    /// sits at three-quarter travel, as it does under the mouse. Requires
    /// `min == 0` and `max == crate::gain::FADER_MAX_GAIN`, the top of the
    /// throw, so a controller, a lane and the mouse fader share one range
    /// (MOO-131).
    Fader,
}

/// One parameter's identity, range, and mapping. The single source of truth:
/// a range written a second time anywhere else is a bug.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDescriptor {
    /// Stable per-kind wire id, carried by `Event::ParamValue`. Automation
    /// will persist these, so never renumber a shipped id — append instead.
    pub id: u32,
    pub name: &'static str,
    /// Suffix for value readouts. Empty when the value is unitless.
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub curve: ParamCurve,
    pub default: f32,
}

impl ParamDescriptor {
    /// Natural units -> 0..1 knob position.
    pub fn to_normalized(&self, natural: f32) -> f32 {
        let clamped = natural.clamp(self.min, self.max);
        let span = self.max - self.min;
        match self.curve {
            ParamCurve::Linear => {
                if span == 0.0 {
                    0.0
                } else {
                    (clamped - self.min) / span
                }
            }
            ParamCurve::Exponential => {
                let ratio = self.max / self.min;
                if ratio <= 1.0 {
                    0.0
                } else {
                    (clamped / self.min).ln() / ratio.ln()
                }
            }
            ParamCurve::Stepped(steps) => {
                if steps <= 1 || span == 0.0 {
                    0.0
                } else {
                    (clamped - self.min) / span
                }
            }
            // The ceiling is snapped rather than taken through the log, which
            // lands a ulp short of it: full throw has to read back as 1.0.
            ParamCurve::Fader if clamped >= self.max => 1.0,
            ParamCurve::Fader => crate::gain::fader_gain_to_position(clamped),
        }
    }

    /// 0..1 knob position -> natural units.
    pub fn from_normalized(&self, norm: f32) -> f32 {
        let t = norm.clamp(0.0, 1.0);
        match self.curve {
            ParamCurve::Linear => self.min + (self.max - self.min) * t,
            ParamCurve::Exponential => {
                let ratio = self.max / self.min;
                if ratio <= 0.0 {
                    self.min
                } else {
                    self.min * ratio.powf(t)
                }
            }
            ParamCurve::Stepped(steps) => {
                if steps <= 1 {
                    self.min
                } else {
                    let last = f32::from(steps - 1);
                    let index = (t * last).round();
                    self.min + (self.max - self.min) * (index / last)
                }
            }
            ParamCurve::Fader => crate::gain::fader_position_to_gain(t).clamp(self.min, self.max),
        }
    }

    /// Clamp a natural value into range, snapping it when stepped.
    pub fn clamp_natural(&self, natural: f32) -> f32 {
        match self.curve {
            ParamCurve::Stepped(_) => self.from_normalized(self.to_normalized(natural)),
            _ => natural.clamp(self.min, self.max),
        }
    }
}

// --- Equalizer -------------------------------------------------------------

/// The EQ has seven musical bands. High- and low-pass filters are separate
/// from this count, matching the way engineers usually think about a channel
/// EQ rather than spending a bell band on cleanup.
pub const EQ_MAX_BANDS: usize = 7;

/// `Event::ParamValue` ids for [`EqParams`].
///
/// **Every band and both pass filters address their own fields.** Until
/// 2026-09-14 there were seven ids and they all meant "the selected target's":
/// `get` and `set` resolved each one through `selected_target`, which was
/// itself id 0 and therefore itself automatable, so a lane on it changed --
/// over time -- which band every other EQ lane referred to. Nothing prevented
/// that and nothing could make it mean anything.
///
/// The reason to fix it is bigger than the EQ. mooloop intends to host CLAP,
/// and a CLAP plugin hands the host N independent parameters, each with a
/// stable id and no context; there is no way to say "the selected band's
/// frequency" to a plugin. This was the one native device whose parameter
/// model a plugin host could not express -- the channel strip's EQ, DS-01,
/// ML-P8 and the modulator modules are all already per-band or per-module.
/// See `docs/plans/eq-v2/01-per-band-parameters.md`.
///
/// ## Ids 0..6 are retired, not reused
///
/// `Band`, `On`, `Freq`, `Gain`, `Q`, `Pass Slope` and `Q Profile` sat at 0..6
/// and projects have saved them. The rule this file states about itself --
/// never renumber a shipped id, append instead -- means they stay spent, and
/// keeping them as aliases for the selected band would preserve exactly the
/// incoherence this removes. So the new space **starts at [`EQ_FIRST`]** and
/// the seven below it address nothing.
///
/// That is what makes this change safe to ship without bumping
/// `FORMAT_VERSION`, and the choice is deliberate. `EqParams`'s persisted
/// *shape* is unchanged -- same bands, same fields, `selected_target` still
/// saved -- so a v1 song's EQ loads and sounds exactly as it did. The only
/// thing that changes meaning is a lane or route addressing an EQ id in 0..6,
/// and because the new ids start at 16 such a lane lands in a **hole**: `get`
/// and `set` answer `None` and it is inert. A version bump would instead
/// refuse the whole document -- every song with no EQ lane in it, and every
/// factory preset already on disk behind a seeding marker that will never
/// re-seed. Refusing a hundred working files to retire seven dead ids is the
/// wrong trade, and leaving a gap under the new base is what buys the
/// alternative.
pub const EQ_FIRST: u32 = 16;

/// Id of band 0's first field. A band's fields are contiguous, so a band's
/// ids are `EQ_BAND_BASE + band * EQ_BAND_STRIDE + field`, the layout
/// `strip_band_param` already uses.
pub const EQ_BAND_BASE: u32 = EQ_FIRST;
/// Ten, not six. The strip took four for four fields and `synth_osc_param`
/// took ten for five; the oscillator's spare room is the one that has been
/// wanted, because appending a field to a band otherwise renumbers every
/// band after it.
pub const EQ_BAND_STRIDE: u32 = 10;
pub const EQ_BAND_ON: u32 = 0;
pub const EQ_BAND_FREQ: u32 = 1;
pub const EQ_BAND_GAIN: u32 = 2;
pub const EQ_BAND_Q: u32 = 3;
pub const EQ_BAND_KIND: u32 = 4;
pub const EQ_BAND_Q_PROFILE: u32 = 5;
/// How many of a band's ten slots are spelled. The rest are the spare room.
pub const EQ_BAND_FIELDS: u32 = 6;

/// Id of the high-pass filter's first field; the low-pass follows a stride
/// later. Pass filters are not bands -- no gain, and a slope where a band has
/// a kind -- so they get their own base rather than a ragged band row.
pub const EQ_PASS_BASE: u32 = EQ_BAND_BASE + EQ_MAX_BANDS as u32 * EQ_BAND_STRIDE;
pub const EQ_PASS_STRIDE: u32 = 10;
pub const EQ_PASS_ON: u32 = 0;
pub const EQ_PASS_FREQ: u32 = 1;
pub const EQ_PASS_Q: u32 = 2;
pub const EQ_PASS_SLOPE: u32 = 3;
pub const EQ_PASS_FIELDS: u32 = 4;
/// The high-pass is pass 0 and the low-pass is pass 1, matching the order the
/// face's selector draws them in and the order they run in.
pub const EQ_HIGH_PASS: usize = 0;
pub const EQ_LOW_PASS: usize = 1;
pub const EQ_PASS_COUNT: usize = 2;

/// The id of one field of one band.
pub const fn eq_band_param(band: usize, field: u32) -> u32 {
    EQ_BAND_BASE + band as u32 * EQ_BAND_STRIDE + field
}

/// The id of one field of one pass filter.
pub const fn eq_pass_param(pass: usize, field: u32) -> u32 {
    EQ_PASS_BASE + pass as u32 * EQ_PASS_STRIDE + field
}

/// Which band and which field an id names, or `None` if it is not a band id.
///
/// The stride's spare slots answer `None` too: an id inside a band's row but
/// past its named fields is reserved, not a sixth field nobody wrote yet.
pub const fn eq_band_of(id: u32) -> Option<(usize, u32)> {
    if id < EQ_BAND_BASE || id >= EQ_PASS_BASE {
        return None;
    }
    let offset = id - EQ_BAND_BASE;
    let field = offset % EQ_BAND_STRIDE;
    if field >= EQ_BAND_FIELDS {
        return None;
    }
    Some(((offset / EQ_BAND_STRIDE) as usize, field))
}

/// Which pass filter and which field an id names, or `None`.
pub const fn eq_pass_of(id: u32) -> Option<(usize, u32)> {
    if id < EQ_PASS_BASE || id >= EQ_PASS_BASE + EQ_PASS_COUNT as u32 * EQ_PASS_STRIDE {
        return None;
    }
    let offset = id - EQ_PASS_BASE;
    let field = offset % EQ_PASS_STRIDE;
    if field >= EQ_PASS_FIELDS {
        return None;
    }
    Some(((offset / EQ_PASS_STRIDE) as usize, field))
}

/// How many parameters the EQ describes: seven bands of six fields and two
/// pass filters of four.
pub const EQ_DESCRIPTOR_COUNT: usize =
    EQ_MAX_BANDS * EQ_BAND_FIELDS as usize + EQ_PASS_COUNT * EQ_PASS_FIELDS as usize;

/// Every descriptor's name, in table order.
///
/// **Generated rather than written out, unlike the channel strip's sixteen
/// band rows**, and the reason the strip gives for longhand -- that a table a
/// reader cannot see is not a table anybody will check a face against -- is
/// what decided the shape here rather than against it. Fifty full
/// `ParamDescriptor` literals is four hundred lines nobody reads; a name and a
/// default per row, with the shape written once, is a table you can check by
/// eye. What a reader actually needs to verify is *which name sits at which
/// id*, and `param_id_freeze_tests` pins all fifty of those explicitly.
const EQ_BAND_NAMES: [[&str; EQ_BAND_FIELDS as usize]; EQ_MAX_BANDS] = [
    ["B1 On", "B1 Freq", "B1 Gain", "B1 Q", "B1 Type", "B1 Q Prof"],
    ["B2 On", "B2 Freq", "B2 Gain", "B2 Q", "B2 Type", "B2 Q Prof"],
    ["B3 On", "B3 Freq", "B3 Gain", "B3 Q", "B3 Type", "B3 Q Prof"],
    ["B4 On", "B4 Freq", "B4 Gain", "B4 Q", "B4 Type", "B4 Q Prof"],
    ["B5 On", "B5 Freq", "B5 Gain", "B5 Q", "B5 Type", "B5 Q Prof"],
    ["B6 On", "B6 Freq", "B6 Gain", "B6 Q", "B6 Type", "B6 Q Prof"],
    ["B7 On", "B7 Freq", "B7 Gain", "B7 Q", "B7 Type", "B7 Q Prof"],
];

const EQ_PASS_NAMES: [[&str; EQ_PASS_FIELDS as usize]; EQ_PASS_COUNT] = [
    ["HP On", "HP Freq", "HP Q", "HP Slope"],
    ["LP On", "LP Freq", "LP Q", "LP Slope"],
];

/// Where the seven bands rest, in hertz.
///
/// The seven-band graphic EQ's own centres -- 63, 160, 400, 1k, 2.5k, 6.3k,
/// 16k -- which is a constant ratio of about 2.5, an octave and a third, per
/// step. A spread rather than a huddle because **every band starts on and
/// every band starts somewhere different**: a bank whose bands 4 to 7 all
/// rested at 1 kHz drew four handles on top of each other, and the first
/// thing anybody did with one was drag it somewhere else.
///
/// Written once here because two tables read it -- [`EQ_BAND_DEFAULTS`],
/// which the descriptors are generated from, and [`EqParams::default`], which
/// is what a freshly added EQ actually runs. They were two hand-written
/// copies of the same seven numbers until 2026-09-15, held together by a test
/// rather than by construction.
pub const EQ_DEFAULT_BAND_HZ: [f32; EQ_MAX_BANDS] =
    [63.0, 160.0, 400.0, 1_000.0, 2_500.0, 6_300.0, 16_000.0];

/// What each band is by default: **the outer two are shelves and the five
/// between them are bells.**
///
/// The low shelf is band 1 and the high shelf is band **7**, which is where a
/// console puts them and where the face draws them. Until 2026-09-15 the high
/// shelf was band 3, sitting a third of the way along a row of seven buttons
/// with four unused bands to the right of it.
pub const EQ_DEFAULT_BAND_KIND: [EqBandKind; EQ_MAX_BANDS] = [
    EqBandKind::LowShelf,
    EqBandKind::Bell,
    EqBandKind::Bell,
    EqBandKind::Bell,
    EqBandKind::Bell,
    EqBandKind::Bell,
    EqBandKind::HighShelf,
];

/// The Q every band and both pass filters rest at: Butterworth, the width
/// that neither rings nor smears, and the slope a shelf runs when its Q knob
/// has not been touched.
pub const EQ_DEFAULT_Q: f32 = 0.707;

/// Every band's resting state, in the order the fields are numbered.
///
/// **Derived rather than written out**, from the same two tables
/// [`EqParams::default`] builds its bands from, so the frequency a descriptor
/// calls band 5's default and the frequency a new EQ puts band 5 at cannot
/// become two different numbers.
/// (`descriptor_defaults_match_the_params_defaults` still checks all fifty,
/// and now checks a derivation rather than a copy.)
const EQ_BAND_DEFAULTS: [[f32; EQ_BAND_FIELDS as usize]; EQ_MAX_BANDS] = {
    // on, freq, gain, q, kind, q profile
    let mut out = [[1.0, 0.0, 0.0, EQ_DEFAULT_Q, 0.0, 0.0]; EQ_MAX_BANDS];
    let mut band = 0;
    while band < EQ_MAX_BANDS {
        out[band][EQ_BAND_FREQ as usize] = EQ_DEFAULT_BAND_HZ[band];
        out[band][EQ_BAND_KIND as usize] = EQ_DEFAULT_BAND_KIND[band].to_index() as f32;
        band += 1;
    }
    out
};

const EQ_PASS_DEFAULTS: [[f32; EQ_PASS_FIELDS as usize]; EQ_PASS_COUNT] = [
    // on, freq, q, slope (1 = Db12)
    [0.0, 30.0, EQ_DEFAULT_Q, 1.0],
    [0.0, 18_000.0, EQ_DEFAULT_Q, 1.0],
];

/// One band field's shape, shared by all seven bands. A range written once is
/// a range that cannot drift between band 3 and band 6.
const fn eq_band_shape(field: u32) -> (&'static str, f32, f32, ParamCurve) {
    match field {
        EQ_BAND_ON => ("", 0.0, 1.0, ParamCurve::Stepped(2)),
        EQ_BAND_FREQ => ("Hz", 20.0, 20_000.0, ParamCurve::Exponential),
        EQ_BAND_GAIN => ("dB", -18.0, 18.0, ParamCurve::Linear),
        EQ_BAND_Q => ("", 0.15, 18.0, ParamCurve::Exponential),
        // Three positions, not the strip's two: a band here offers bell, low
        // shelf and high shelf, which is the device for wanting a high shelf
        // on a low band. `strip_band_shelf` says why the strip refuses that.
        EQ_BAND_KIND => ("", 0.0, 2.0, ParamCurve::Stepped(3)),
        _ => ("", 0.0, 1.0, ParamCurve::Stepped(2)),
    }
}

const fn eq_pass_shape(field: u32) -> (&'static str, f32, f32, ParamCurve) {
    match field {
        EQ_PASS_ON => ("", 0.0, 1.0, ParamCurve::Stepped(2)),
        EQ_PASS_FREQ => ("Hz", 20.0, 20_000.0, ParamCurve::Exponential),
        EQ_PASS_Q => ("", 0.15, 18.0, ParamCurve::Exponential),
        _ => (
            "",
            0.0,
            EQ_SLOPE_COUNT as f32 - 1.0,
            ParamCurve::Stepped(EQ_SLOPE_COUNT as u16),
        ),
    }
}

const fn eq_descriptors() -> [ParamDescriptor; EQ_DESCRIPTOR_COUNT] {
    let blank = ParamDescriptor {
        id: 0,
        name: "",
        unit: "",
        min: 0.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    };
    let mut out = [blank; EQ_DESCRIPTOR_COUNT];
    let mut index = 0;
    let mut band = 0;
    while band < EQ_MAX_BANDS {
        let mut field = 0;
        while field < EQ_BAND_FIELDS as usize {
            let (unit, min, max, curve) = eq_band_shape(field as u32);
            out[index] = ParamDescriptor {
                id: eq_band_param(band, field as u32),
                name: EQ_BAND_NAMES[band][field],
                unit,
                min,
                max,
                curve,
                default: EQ_BAND_DEFAULTS[band][field],
            };
            index += 1;
            field += 1;
        }
        band += 1;
    }
    let mut pass = 0;
    while pass < EQ_PASS_COUNT {
        let mut field = 0;
        while field < EQ_PASS_FIELDS as usize {
            let (unit, min, max, curve) = eq_pass_shape(field as u32);
            out[index] = ParamDescriptor {
                id: eq_pass_param(pass, field as u32),
                name: EQ_PASS_NAMES[pass][field],
                unit,
                min,
                max,
                curve,
                default: EQ_PASS_DEFAULTS[pass][field],
            };
            index += 1;
            field += 1;
        }
        pass += 1;
    }
    out
}

static EQ_DESCRIPTORS: [ParamDescriptor; EQ_DESCRIPTOR_COUNT] = eq_descriptors();

/// The low end of the frequency axis every EQ response plot draws on.
///
/// Here rather than in the display or the publisher because four places need
/// the same answer and they are in three crates: `strip_row` and
/// `effect_slot_row` normalize a band's hertz onto it, `mooloop_dsp` samples
/// the bank along it, and the markup reads the result back by position. It
/// was `(hz / 20.0).ln() / 1000.0f32.ln()` written twice in `mooloop-ui`
/// until 2026-09-14, which is the shape `AGENTS.md` opens on: an axis is a
/// policy, and a policy spelled twice is one that drifts.
///
/// It is a *display* convention rather than a parameter range -- a band's
/// frequency descriptor runs 20 Hz to 20 kHz too, and the coincidence is not
/// load-bearing: a plot that stops at 20 kHz is right whatever the knob can
/// reach, because that is where hearing stops.
pub const EQ_PLOT_MIN_HZ: f32 = 20.0;
/// The high end of that axis.
pub const EQ_PLOT_MAX_HZ: f32 = 20_000.0;

/// Where `frequency_hz` sits along the response plot's axis, 0 at
/// [`EQ_PLOT_MIN_HZ`] and 1 at [`EQ_PLOT_MAX_HZ`], logarithmically.
pub fn eq_plot_position(frequency_hz: f32) -> f32 {
    (frequency_hz.max(1e-3) / EQ_PLOT_MIN_HZ).ln() / (EQ_PLOT_MAX_HZ / EQ_PLOT_MIN_HZ).ln()
}

/// The frequency at `position` along that axis. The inverse of
/// [`eq_plot_position`], and what a response sampler walks.
pub fn eq_plot_frequency(position: f32) -> f32 {
    EQ_PLOT_MIN_HZ * (EQ_PLOT_MAX_HZ / EQ_PLOT_MIN_HZ).powf(position.clamp(0.0, 1.0))
}

/// A band's response topology. The first and last default bands are shelves;
/// any active interior band is a peaking filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandKind {
    #[default]
    Bell,
    LowShelf,
    HighShelf,
}

impl EqBandKind {
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::LowShelf,
            2 => Self::HighShelf,
            _ => Self::Bell,
        }
    }
    /// `const` because [`EQ_BAND_DEFAULTS`] is generated at compile time and
    /// reads a band's kind through here, rather than spelling the same
    /// integer a second time in a table of floats.
    pub const fn to_index(self) -> i32 {
        match self {
            Self::Bell => 0,
            Self::LowShelf => 1,
            Self::HighShelf => 2,
        }
    }
}

/// Bell bandwidth profile. Proportional Q is the familiar API behavior: a
/// larger cut or boost produces a narrower, more assertive curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqQProfile {
    #[default]
    Constant,
    Proportional,
}

impl EqQProfile {
    pub fn from_index(index: i32) -> Self {
        if index >= 1 {
            Self::Proportional
        } else {
            Self::Constant
        }
    }
    pub fn to_index(self) -> i32 {
        match self {
            Self::Constant => 0,
            Self::Proportional => 1,
        }
    }
}

/// The Q a band is actually running, after its profile.
///
/// Proportional Q widens the assertion as well as the curve: a boost of
/// 12 dB doubles the Q, so the band narrows as it is pushed. Here rather
/// than in the DSP because three callers need the same answer and they are
/// in different crates -- `EqEffect`, the channel strip's own bank, and
/// every response display that has to plot the curve that is *running*
/// rather than the one the knobs imply. The law written twice is the law
/// that drifts, and the copy that drifts is the one deciding what is heard.
pub fn eq_effective_q(q: f32, gain_db: f32, profile: EqQProfile) -> f32 {
    let boost = match profile {
        EqQProfile::Constant => 1.0,
        EqQProfile::Proportional => 1.0 + gain_db.abs() / 12.0,
    };
    (q * boost).clamp(0.15, 30.0)
}

/// One band in the seven-band EQ bank.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqBand {
    pub enabled: bool,
    pub kind: EqBandKind,
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
    pub q_profile: EqQProfile,
}

impl EqBand {
    pub const fn bell(frequency_hz: f32) -> Self {
        Self {
            enabled: false,
            kind: EqBandKind::Bell,
            frequency_hz,
            gain_db: 0.0,
            q: EQ_DEFAULT_Q,
            q_profile: EqQProfile::Constant,
        }
    }
}

/// Octave slope for dedicated high- and low-pass filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqSlope {
    #[default]
    Db6,
    Db12,
    Db18,
    Db24,
    Db36,
}

impl EqSlope {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Db12,
            2 => Self::Db18,
            3 => Self::Db24,
            4 => Self::Db36,
            _ => Self::Db6,
        }
    }
    pub fn to_index(self) -> i32 {
        match self {
            Self::Db6 => 0,
            Self::Db12 => 1,
            Self::Db18 => 2,
            Self::Db24 => 3,
            Self::Db36 => 4,
        }
    }
    pub fn stages(self) -> usize {
        match self {
            Self::Db6 => 1,
            Self::Db12 => 2,
            Self::Db18 => 3,
            Self::Db24 => 4,
            Self::Db36 => 6,
        }
    }

    /// What this slope is, in decibels per octave, and what a face should
    /// call it.
    ///
    /// **The variant names are half the slope they name, and they are kept
    /// anyway.** A stage is `Biquad::pass`, which is the cookbook's
    /// *second-order* section -- 12 dB per octave -- so `Db6` runs one of
    /// them and rolls off at twelve, and `Db36` runs six and rolls off at
    /// seventy-two. The selector on the EQ's face read "6 12 18 24 36" over a
    /// bank doing 12/24/36/48/72 from the day it shipped, which is this
    /// codebase's recurring fault in its plainest form: a control saying
    /// something the engine is not doing.
    ///
    /// The names are the *persisted* spelling -- `serde` writes `"db6"` --
    /// so renaming the variants would either refuse every saved project or
    /// silently re-map one slope to another, to correct a label. The label is
    /// what was wrong and the label is what moved. `docs/LOOSE_ENDS.md`
    /// carries the rename for whenever a `FORMAT_VERSION` bump happens for a
    /// reason worth having one.
    pub fn db_per_octave(self) -> u32 {
        self.stages() as u32 * PASS_STAGE_DB_PER_OCTAVE
    }

    /// Every slope, in index order, for a face that draws them all.
    pub fn all() -> [Self; EQ_SLOPE_COUNT] {
        [Self::Db6, Self::Db12, Self::Db18, Self::Db24, Self::Db36]
    }
}

/// One `Biquad::pass` stage is second-order, and second-order is 12 dB per
/// octave. The one place that arithmetic is stated.
pub const PASS_STAGE_DB_PER_OCTAVE: u32 = 12;

/// How many slopes a pass filter offers.
pub const EQ_SLOPE_COUNT: usize = 5;

/// Independent low- or high-pass cleanup filter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqPassFilter {
    pub enabled: bool,
    pub frequency_hz: f32,
    pub q: f32,
    pub slope: EqSlope,
}

impl EqPassFilter {
    const fn high_pass() -> Self {
        Self {
            enabled: false,
            frequency_hz: 30.0,
            q: EQ_DEFAULT_Q,
            slope: EqSlope::Db12,
        }
    }
    const fn low_pass() -> Self {
        Self {
            enabled: false,
            frequency_hz: 18_000.0,
            q: EQ_DEFAULT_Q,
            slope: EqSlope::Db12,
        }
    }
}

/// Full persisted EQ state. `selected_target` is retained so reopening an EQ
/// returns to the band the user was shaping, without treating it as a seventh
/// automation destination.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EqParams {
    pub bands: [EqBand; EQ_MAX_BANDS],
    pub high_pass: EqPassFilter,
    pub low_pass: EqPassFilter,
    #[serde(default = "default_eq_selected_target")]
    pub selected_target: u8,
    #[serde(default)]
    pub analyzer_enabled: bool,
}

const fn default_eq_selected_target() -> u8 {
    1
}

impl Default for EqParams {
    /// **All seven bands, on, spread across the band, flat.**
    ///
    /// Three of the seven were on and the other four were parked at 1 kHz
    /// until 2026-09-15, which made a seven-band EQ a three-band EQ with four
    /// handles hidden under band 2's. Every band at 0 dB is transparent --
    /// a bell at unity gain is the identity filter -- so what this costs is
    /// seven biquads per channel instead of three, and what it buys is a face
    /// where every handle on the plot is the handle it looks like.
    fn default() -> Self {
        let mut bands = [EqBand::bell(EQ_DEFAULT_BAND_HZ[0]); EQ_MAX_BANDS];
        for (index, band) in bands.iter_mut().enumerate() {
            *band = EqBand {
                enabled: true,
                kind: EQ_DEFAULT_BAND_KIND[index],
                ..EqBand::bell(EQ_DEFAULT_BAND_HZ[index])
            };
        }
        Self {
            bands,
            high_pass: EqPassFilter::high_pass(),
            low_pass: EqPassFilter::low_pass(),
            selected_target: default_eq_selected_target(),
            analyzer_enabled: false,
        }
    }
}

impl EqParams {
    pub const HIGH_PASS_TARGET: usize = EQ_MAX_BANDS;
    pub const LOW_PASS_TARGET: usize = EQ_MAX_BANDS + 1;

    /// One pass filter by index, high-pass first. Indexed rather than named
    /// because [`eq_pass_of`] hands back a number, and the face's selector
    /// draws them in this order too.
    pub fn pass(&self, index: usize) -> Option<&EqPassFilter> {
        match index {
            EQ_HIGH_PASS => Some(&self.high_pass),
            EQ_LOW_PASS => Some(&self.low_pass),
            _ => None,
        }
    }

    pub fn pass_mut(&mut self, index: usize) -> Option<&mut EqPassFilter> {
        match index {
            EQ_HIGH_PASS => Some(&mut self.high_pass),
            EQ_LOW_PASS => Some(&mut self.low_pass),
            _ => None,
        }
    }

    /// Which pass filter a *selection* is showing, for the face. The
    /// high-pass unless the low-pass is the one selected.
    pub fn selected_pass(&self, target: usize) -> &EqPassFilter {
        if target == Self::LOW_PASS_TARGET {
            &self.low_pass
        } else {
            &self.high_pass
        }
    }

    pub fn selected_target(self) -> usize {
        usize::from(self.selected_target).min(Self::LOW_PASS_TARGET)
    }

    pub fn selected_band(self) -> Option<EqBand> {
        self.bands.get(self.selected_target()).copied()
    }

    /// Which control set the face is showing, and nothing else.
    ///
    /// It was id 0 until 2026-09-14 and therefore automatable, which is the
    /// incoherence this device was rebuilt to remove: a lane on it changed,
    /// over time, which band every other EQ lane referred to. It is still
    /// persisted -- reopening an EQ should return to the band you were
    /// shaping -- and it is no longer reachable by an automation lane,
    /// because it is no longer a parameter.
    pub fn set_selected_target(&mut self, target: usize) {
        self.selected_target = target.min(Self::LOW_PASS_TARGET) as u8;
    }

    /// The ids a face's control writes to for the target it is showing, or
    /// `None` where that target has no such control -- a pass filter has no
    /// gain and no Q profile, and a band has no slope.
    ///
    /// The face is one control set over a selection and stays that way; what
    /// changed is that the selection is resolved *here*, to a real per-band
    /// id, instead of being a parameter the DSP had to consult.
    pub fn id_for_selected(target: usize, control: EqFaceControl) -> Option<u32> {
        if target < EQ_MAX_BANDS {
            let field = match control {
                EqFaceControl::Enabled => EQ_BAND_ON,
                EqFaceControl::Frequency => EQ_BAND_FREQ,
                EqFaceControl::Gain => EQ_BAND_GAIN,
                EqFaceControl::Q => EQ_BAND_Q,
                EqFaceControl::QProfile => EQ_BAND_Q_PROFILE,
                EqFaceControl::PassSlope => return None,
            };
            return Some(eq_band_param(target, field));
        }
        let pass = if target == Self::LOW_PASS_TARGET {
            EQ_LOW_PASS
        } else {
            EQ_HIGH_PASS
        };
        let field = match control {
            EqFaceControl::Enabled => EQ_PASS_ON,
            EqFaceControl::Frequency => EQ_PASS_FREQ,
            EqFaceControl::Q => EQ_PASS_Q,
            EqFaceControl::PassSlope => EQ_PASS_SLOPE,
            EqFaceControl::Gain | EqFaceControl::QProfile => return None,
        };
        Some(eq_pass_param(pass, field))
    }
}

/// A switch as a parameter value. The descriptors declare `Stepped(2)` over
/// 0..1, so the two sides are exactly 0 and 1 rather than anything rounded.
const fn bool_to_f32(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

/// The EQ face's control set, which is a *view* of one target rather than a
/// parameter space.
///
/// The face draws one Freq, one Gain, one Q and so on, and a selector saying
/// which band they are aimed at -- and it should: seven duplicated control
/// sets is not an interface. This enum is that view's vocabulary, and
/// [`EqParams::id_for_selected`] is the only place a selection is turned back
/// into an id. Ordered to match the face's own control indices, which is what
/// `eq_face_control` in `mooloop-ui` decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqFaceControl {
    Enabled,
    Frequency,
    Gain,
    Q,
    PassSlope,
    QProfile,
}

/// How many control indices the EQ face uses, counting the band selector at
/// 0. The width of the modulation arrays published for this device.
pub const EQ_FACE_CONTROLS: usize = 7;

impl EqFaceControl {
    /// **The face's control indices are the ids this device used to have.**
    ///
    /// Deliberately, and it is what let `eq-device.slint` and the rack's
    /// wiring survive this change untouched: `modulation-allowed[2]` is the
    /// Freq knob's arc, and Freq was id 2. The face was always a view over a
    /// selection and still is; only what sits underneath it changed, so the
    /// view's own numbering had no reason to move.
    ///
    /// Index 0 was the band selector. It is not a parameter any more, so it
    /// is not a control here either -- the caller handles it as what it is,
    /// a change of which target the face is showing.
    pub const fn from_face_index(index: u32) -> Option<Self> {
        match index {
            1 => Some(EqFaceControl::Enabled),
            2 => Some(EqFaceControl::Frequency),
            3 => Some(EqFaceControl::Gain),
            4 => Some(EqFaceControl::Q),
            5 => Some(EqFaceControl::PassSlope),
            6 => Some(EqFaceControl::QProfile),
            _ => None,
        }
    }

    pub const fn face_index(self) -> u32 {
        match self {
            EqFaceControl::Enabled => 1,
            EqFaceControl::Frequency => 2,
            EqFaceControl::Gain => 3,
            EqFaceControl::Q => 4,
            EqFaceControl::PassSlope => 5,
            EqFaceControl::QProfile => 6,
        }
    }

    /// The band selector's index, which is a control the face has and the
    /// parameter space does not.
    pub const SELECTOR: u32 = 0;
}

// --- Filter ----------------------------------------------------------------

/// `Event::ParamValue` ids for [`FilterParams`].
pub const FILTER_PARAM_CUTOFF_HZ: u32 = 0;
pub const FILTER_PARAM_RESONANCE: u32 = 1;
pub const FILTER_PARAM_MODE: u32 = 2;
pub const FILTER_PARAM_SLOPE: u32 = 3;
pub const FILTER_PARAM_DRIVE: u32 = 4;

static FILTER_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: FILTER_PARAM_CUTOFF_HZ,
        name: "Cutoff",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        curve: ParamCurve::Exponential,
        default: 8_000.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_RESONANCE,
        name: "Reso",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_MODE,
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Stepped(3),
        default: 0.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_SLOPE,
        name: "Slope",
        unit: "dB/oct",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 0.0,
    },
    ParamDescriptor {
        id: FILTER_PARAM_DRIVE,
        name: "Drive",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Filter mode: low-pass attenuates above the cutoff, high-pass below it,
/// and band-pass isolates the region around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    #[default]
    LowPass,
    BandPass,
    HighPass,
}

impl FilterMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::BandPass,
            2 => Self::HighPass,
            _ => Self::LowPass,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::LowPass => 0,
            Self::BandPass => 1,
            Self::HighPass => 2,
        }
    }
}

/// Available state-variable filter cascades. Each SVF is a 12 dB/oct stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterSlope {
    #[default]
    Db12,
    Db24,
}

impl FilterSlope {
    pub fn from_index(index: i32) -> Self {
        if index >= 1 {
            Self::Db24
        } else {
            Self::Db12
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Db12 => 0,
            Self::Db24 => 1,
        }
    }
}

/// Parameters for the filter effect (`FilterEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    /// Cutoff frequency in Hz, clamped by the DSP to [20, sample_rate * 0.45].
    pub cutoff_hz: f32,
    /// Resonance in `[0, 1]`, approaching self-oscillation at the top.
    pub resonance: f32,
    pub mode: FilterMode,
    #[serde(default)]
    pub slope: FilterSlope,
    /// Compensated soft-saturation amount applied before the filter cascade.
    #[serde(default)]
    pub drive: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 8_000.0,
            resonance: 0.0,
            mode: FilterMode::default(),
            slope: FilterSlope::default(),
            drive: 0.0,
        }
    }
}

// --- Drive -----------------------------------------------------------------

/// `Event::ParamValue` ids for [`DriveParams`].
pub const DRIVE_PARAM_DRIVE: u32 = 0;
pub const DRIVE_PARAM_CURVE: u32 = 1;
pub const DRIVE_PARAM_TONE: u32 = 2;
pub const DRIVE_PARAM_MIX: u32 = 3;
pub const DRIVE_PARAM_OUTPUT: u32 = 4;

static DRIVE_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: DRIVE_PARAM_DRIVE,
        name: "Drive",
        unit: "x",
        min: 1.0,
        max: 64.0,
        curve: ParamCurve::Exponential,
        default: 2.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_CURVE,
        name: "Curve",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_TONE,
        name: "Tone",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: DRIVE_PARAM_OUTPUT,
        name: "Out",
        unit: "x",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

/// Shaping curve. `Soft` and `Tape` compress gently, `Hard` clips, and `Fold`
/// wraps past full scale into the inharmonic territory this instrument is
/// actually for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveCurve {
    #[default]
    Soft,
    Hard,
    Fold,
    Tape,
}

impl DriveCurve {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Hard,
            2 => Self::Fold,
            3 => Self::Tape,
            _ => Self::Soft,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Soft => 0,
            Self::Hard => 1,
            Self::Fold => 2,
            Self::Tape => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "SOFT",
            Self::Hard => "HARD",
            Self::Fold => "FOLD",
            Self::Tape => "TAPE",
        }
    }
}

/// Parameters for the drive/saturation effect (`DriveEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DriveParams {
    /// Linear input gain into the shaper.
    pub drive: f32,
    pub curve: DriveCurve,
    /// Post-shaper spectral tilt in `[-1, 1]`: negative darkens, positive
    /// brightens, zero is flat.
    pub tone: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
    /// Linear output trim, applied after the blend.
    pub output: f32,
}

impl Default for DriveParams {
    fn default() -> Self {
        Self {
            drive: 2.0,
            curve: DriveCurve::default(),
            tone: 0.0,
            mix: 1.0,
            output: 1.0,
        }
    }
}

// --- Preamp ----------------------------------------------------------------

/// `Event::ParamValue` ids for [`PreampParams`].
pub const PREAMP_PARAM_DRIVE_DB: u32 = 0;
pub const PREAMP_PARAM_VOICING: u32 = 1;
pub const PREAMP_PARAM_MIX: u32 = 2;
pub const PREAMP_PARAM_OUTPUT_DB: u32 = 3;

static PREAMP_DESCRIPTORS: [ParamDescriptor; 4] = [
    ParamDescriptor {
        id: PREAMP_PARAM_DRIVE_DB,
        name: "Drive",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: PREAMP_PARAM_VOICING,
        name: "Voicing",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
    ParamDescriptor {
        id: PREAMP_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: PREAMP_PARAM_OUTPUT_DB,
        name: "Output",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Which voicing the stage runs, named for the sound rather than for any
/// hardware. `mooloop_dsp::preamp` holds what each one is made of; this is
/// only the choice, which is what a project has to persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreampVoicing {
    /// Uncoloured, and the identity: the stage is skipped outright, so a
    /// `Preamp` in this voicing is bit-identical to no device at all except
    /// for its own drive and output trims.
    #[default]
    Moo,
    /// Fast, tight, controlled. Low distortion and mostly odd-order.
    Grip,
    /// Forward and thick, both harmonic orders present.
    Punch,
    /// Transformer warmth: even-order dominant, and the one whose distortion
    /// is the point.
    Iron,
}

impl PreampVoicing {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Grip,
            2 => Self::Punch,
            3 => Self::Iron,
            _ => Self::Moo,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Moo => 0,
            Self::Grip => 1,
            Self::Punch => 2,
            Self::Iron => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Moo => "MOO",
            Self::Grip => "GRIP",
            Self::Punch => "PUNCH",
            Self::Iron => "IRON",
        }
    }
}

/// Parameters for the preamp effect (`PreampEffect` in `mooloop-dsp`).
///
/// Drive and output are in dB rather than as linear gains, unlike
/// [`DriveParams`]. They are the reason this device is insertable at all --
/// a chain had nowhere to automate gain -- and an automation lane drawn in
/// dB is the one a person can read.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PreampParams {
    /// Gain into the curve. The voicing's profile is authored at the -12 dBFS
    /// operating level, so 0 dB here is where it measures true.
    pub drive_db: f32,
    pub voicing: PreampVoicing,
    /// Dry/wet blend in `[0, 1]`, for parallel saturation.
    pub mix: f32,
    /// Output trim, applied after the blend.
    pub output_db: f32,
    /// Whether the band display is running.
    ///
    /// Not a [`ParamDescriptor`] parameter, for the same reason the EQ's
    /// analyzer is not one: it is a view setting that happens to be worth
    /// saving, and automating it would automate nothing audible. It gates
    /// both halves at once -- the engine subscription the UI takes out, and
    /// the two analyzers the node otherwise runs for nobody.
    pub display_enabled: bool,
}

impl Default for PreampParams {
    fn default() -> Self {
        Self {
            drive_db: 0.0,
            voicing: PreampVoicing::default(),
            mix: 1.0,
            output_db: 0.0,
            display_enabled: false,
        }
    }
}

// --- Bitcrush --------------------------------------------------------------

/// `Event::ParamValue` ids for [`BitcrushParams`].
pub const BITCRUSH_PARAM_BITS: u32 = 0;
pub const BITCRUSH_PARAM_DOWNSAMPLE: u32 = 1;
pub const BITCRUSH_PARAM_MIX: u32 = 2;
pub const BITCRUSH_PARAM_STYLE: u32 = 3;

static BITCRUSH_DESCRIPTORS: [ParamDescriptor; 4] = [
    ParamDescriptor {
        id: BITCRUSH_PARAM_BITS,
        name: "Bits",
        unit: "bit",
        min: 1.0,
        max: 16.0,
        curve: ParamCurve::Linear,
        default: 16.0,
    },
    ParamDescriptor {
        id: BITCRUSH_PARAM_DOWNSAMPLE,
        name: "Rate",
        unit: "x",
        min: 1.0,
        max: 64.0,
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    ParamDescriptor {
        id: BITCRUSH_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: BITCRUSH_PARAM_STYLE,
        name: "Style",
        unit: "",
        min: 0.0,
        max: 3.0,
        curve: ParamCurve::Stepped(4),
        default: 0.0,
    },
];

/// Which quantization/decimation math the bitcrusher runs. All styles share
/// the `bits` and `downsample` inputs; they differ in how coarsely the held
/// sample is snapped and how the hold is drawn between latch points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcrushStyle {
    /// Round-to-nearest quantization of a hard-held sample — the classic
    /// crusher, and the behaviour everything shipped with until styles
    /// existed.
    #[default]
    Crush,
    /// TPDF noise added before quantization, so the depth collapses into
    /// modulated noise instead of hard distortion. The signal hides inside
    /// the grain rather than breaking up.
    Dither,
    /// Companding quantizer: fine steps near silence, coarse at peaks, so
    /// quiet material keeps its detail while loud material crushes hardest.
    Mu,
    /// Linear interpolation between latch points instead of a hard hold —
    /// the same sample rate, but a softer, duller alias character.
    Glide,
}

impl BitcrushStyle {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Dither,
            2 => Self::Mu,
            3 => Self::Glide,
            _ => Self::Crush,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Crush => 0,
            Self::Dither => 1,
            Self::Mu => 2,
            Self::Glide => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Crush => "CRUSH",
            Self::Dither => "DITH",
            Self::Mu => "MU",
            Self::Glide => "GLIDE",
        }
    }
}

/// Parameters for the bitcrush effect (`BitcrushEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BitcrushParams {
    /// Quantization depth in bits. Fractional values are meaningful — the
    /// step size is continuous, so this can be swept without zippering.
    pub bits: f32,
    /// Sample-and-hold length in input samples. 1.0 holds nothing.
    pub downsample: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
    /// Which crusher math runs (see [`BitcrushStyle`]).
    #[serde(default)]
    pub style: BitcrushStyle,
}

impl Default for BitcrushParams {
    fn default() -> Self {
        Self {
            bits: 16.0,
            downsample: 1.0,
            mix: 1.0,
            style: BitcrushStyle::default(),
        }
    }
}

// --- Delay -----------------------------------------------------------------

/// `Event::ParamValue` ids for [`DelayParams`].
pub const DELAY_PARAM_TIME_MS: u32 = 0;
pub const DELAY_PARAM_FEEDBACK: u32 = 1;
pub const DELAY_PARAM_MODE: u32 = 2;
pub const DELAY_PARAM_CROSS: u32 = 3;
pub const DELAY_PARAM_TONE: u32 = 4;
pub const DELAY_PARAM_MIX: u32 = 5;

/// Longest delay time, and therefore the ring the effect allocates per slot:
/// two seconds of stereo `f32` is about 768 kB at 48 kHz -- 750 KiB, not 768
/// of them; 2 x 48_000 x 2 x 4 bytes.
pub const DELAY_MAX_TIME_MS: f32 = 2_000.0;

static DELAY_DESCRIPTORS: [ParamDescriptor; 6] = [
    ParamDescriptor {
        id: DELAY_PARAM_TIME_MS,
        name: "Time",
        unit: "ms",
        min: 1.0,
        max: DELAY_MAX_TIME_MS,
        curve: ParamCurve::Exponential,
        default: 375.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_FEEDBACK,
        name: "Fdbk",
        unit: "",
        // Stops short of 1.0: unity feedback with any damping still runs away
        // once the wet path is summed back in.
        min: 0.0,
        max: 0.98,
        curve: ParamCurve::Linear,
        default: 0.35,
    },
    ParamDescriptor {
        id: DELAY_PARAM_MODE,
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 2.0,
        curve: ParamCurve::Stepped(3),
        default: 0.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_CROSS,
        name: "Cross",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: DELAY_PARAM_TONE,
        name: "Tone",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.6,
    },
    ParamDescriptor {
        id: DELAY_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.35,
    },
];

/// How the read head responds when the delay time moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayMode {
    /// Crossfade to the new time. The repeats keep their pitch.
    #[default]
    Digital,
    /// Glide to the new time, so the buffered audio repitches on the way —
    /// the tape-delay behavior.
    Tape,
    /// Read the recent history backwards in windows the length of the delay
    /// time.
    Reverse,
}

impl DelayMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Tape,
            2 => Self::Reverse,
            _ => Self::Digital,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Digital => 0,
            Self::Tape => 1,
            Self::Reverse => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Digital => "DIGI",
            Self::Tape => "TAPE",
            Self::Reverse => "REV",
        }
    }
}

/// Parameters for the delay effect (`DelayEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DelayParams {
    pub time_ms: f32,
    /// Whether `time_ms` is derived from [`Self::time_division`] and the
    /// project's current BPM. DSP still receives only the resolved ms value.
    #[serde(default)]
    pub tempo_sync: bool,
    /// Persist the musical input value so the delay continues tracking tempo
    /// changes after a project round-trip.
    #[serde(default)]
    pub time_division: crate::ModTimeDivision,
    /// Feedback gain in `[0, 0.98]`.
    pub feedback: f32,
    pub mode: DelayMode,
    /// How much of the feedback path crosses to the other channel. At 1.0 the
    /// repeats alternate sides (ping-pong).
    pub cross: f32,
    /// Damping of the feedback path in `[0, 1]`: 0 darkens each repeat
    /// heavily, 1 leaves it open.
    pub tone: f32,
    /// Dry/wet blend in `[0, 1]`.
    pub mix: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            time_ms: 375.0,
            tempo_sync: false,
            time_division: crate::ModTimeDivision::default(),
            feedback: 0.35,
            mode: DelayMode::default(),
            cross: 0.0,
            tone: 0.6,
            mix: 0.35,
        }
    }
}

// --- Modulation ------------------------------------------------------------

/// `Event::ParamValue` ids for [`ModulationParams`].
/// The one LFO every modulation mode runs from. Named rather than written
/// into the descriptor, because a synced division has to be clamped to the
/// same ceiling and a second copy of `12.0` is how the two come to disagree.
pub const MODULATION_MIN_RATE_HZ: f32 = 0.02;
pub const MODULATION_MAX_RATE_HZ: f32 = 12.0;

pub const MODULATION_PARAM_MODE: u32 = 0;
pub const MODULATION_PARAM_RATE_HZ: u32 = 1;
pub const MODULATION_PARAM_DEPTH: u32 = 2;
pub const MODULATION_PARAM_COLOR: u32 = 3;
pub const MODULATION_PARAM_FEEDBACK: u32 = 4;
pub const MODULATION_PARAM_SPREAD: u32 = 5;
pub const MODULATION_PARAM_TONE: u32 = 6;
pub const MODULATION_PARAM_STAGES: u32 = 7;

static MODULATION_DESCRIPTORS: [ParamDescriptor; 8] = [
    ParamDescriptor {
        id: MODULATION_PARAM_MODE,
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 4.0,
        curve: ParamCurve::Stepped(5),
        default: 0.0,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_RATE_HZ,
        name: "Rate",
        unit: "Hz",
        min: MODULATION_MIN_RATE_HZ,
        max: MODULATION_MAX_RATE_HZ,
        curve: ParamCurve::Exponential,
        default: 0.35,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_DEPTH,
        name: "Depth",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.45,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_COLOR,
        name: "Color",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.45,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_FEEDBACK,
        name: "Feedback",
        unit: "",
        min: -0.92,
        max: 0.92,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_SPREAD,
        name: "Spread",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.65,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_TONE,
        name: "Tone",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.75,
    },
    ParamDescriptor {
        id: MODULATION_PARAM_STAGES,
        name: "Stages",
        unit: "",
        min: 4.0,
        max: 12.0,
        curve: ParamCurve::Stepped(5),
        default: 8.0,
    },
];

/// Algorithms exposed by the unified modulation processor. The first four
/// share a modulated delay line; phaser uses a compact all-pass cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulationMode {
    #[default]
    Chorus,
    Flange,
    Phaser,
    Ensemble,
    Adt,
}

impl ModulationMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Flange,
            2 => Self::Phaser,
            3 => Self::Ensemble,
            4 => Self::Adt,
            _ => Self::Chorus,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Chorus => 0,
            Self::Flange => 1,
            Self::Phaser => 2,
            Self::Ensemble => 3,
            Self::Adt => 4,
        }
    }
}

/// Parameters for the shared modulation processor. `color` deliberately has
/// one stable wire identity while each algorithm names it musically on the
/// face (delay, sweep centre, or tape age).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModulationParams {
    pub mode: ModulationMode,
    pub rate_hz: f32,
    /// Whether [`Self::rate_hz`] is derived from [`Self::rate_division`] and
    /// the project's current BPM. The DSP still receives only the resolved
    /// rate, exactly as the delay receives only a resolved millisecond value.
    #[serde(default)]
    pub tempo_sync: bool,
    /// Persisted so a synced sweep keeps following the tempo across a save.
    #[serde(default)]
    pub rate_division: crate::ModTimeDivision,
    pub depth: f32,
    pub color: f32,
    pub feedback: f32,
    pub spread: f32,
    pub tone: f32,
    pub stages: u8,
}

impl ModulationParams {
    /// The rate a synced division resolves to, clamped to what this device
    /// can run: a 64th triplet at 120 BPM asks for 48 Hz against a 12 Hz
    /// ceiling, and the whole grid is offered because the slow end of it is
    /// where a synced flanger is worth having.
    pub fn synced_rate_hz(&self, bpm: f64) -> f32 {
        self.rate_division
            .rate_hz(bpm)
            .clamp(MODULATION_MIN_RATE_HZ, MODULATION_MAX_RATE_HZ)
    }
}

impl Default for ModulationParams {
    fn default() -> Self {
        Self {
            mode: ModulationMode::default(),
            rate_hz: 0.35,
            tempo_sync: false,
            rate_division: crate::ModTimeDivision::Whole,
            depth: 0.45,
            color: 0.45,
            feedback: 0.0,
            spread: 0.65,
            tone: 0.75,
            stages: 8,
        }
    }
}

// --- Reverb ----------------------------------------------------------------

/// `Event::ParamValue` ids for [`ReverbParams`].
///
/// These start at 8 because ids 0..=7 are **retired**: they belonged to the
/// generated-room convolution reverb this device replaced (shape, material,
/// width/depth/height in metres, decay, and a capture point). A shipped id
/// may never change meaning — see `docs/MODULATION.md` — so a saved
/// route or automation lane aimed at the old "Mic X" resolves to no
/// descriptor and stays inert, instead of silently becoming Diffusion.
pub const REVERB_PARAM_SIZE: u32 = 8;
pub const REVERB_PARAM_DECAY_S: u32 = 9;
pub const REVERB_PARAM_DAMPING: u32 = 10;
pub const REVERB_PARAM_PREDELAY_MS: u32 = 11;
pub const REVERB_PARAM_DIFFUSION: u32 = 12;
pub const REVERB_PARAM_WIDTH: u32 = 13;
pub const REVERB_PARAM_MODULATION: u32 = 14;
pub const REVERB_PARAM_LOW_CUT_HZ: u32 = 15;

static REVERB_DESCRIPTORS: [ParamDescriptor; 8] = [
    ParamDescriptor {
        id: REVERB_PARAM_SIZE,
        name: "Size",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.5,
    },
    ParamDescriptor {
        id: REVERB_PARAM_DECAY_S,
        name: "Decay",
        unit: "s",
        min: 0.2,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 2.4,
    },
    ParamDescriptor {
        id: REVERB_PARAM_DAMPING,
        name: "Damp",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.38,
    },
    // Exponential needs a positive minimum, and 1 ms is already below the
    // point where a pre-delay reads as a separate arrival, so it serves as
    // this control's "off" rather than a missing zero.
    ParamDescriptor {
        id: REVERB_PARAM_PREDELAY_MS,
        name: "Pre",
        unit: "ms",
        min: 1.0,
        max: 200.0,
        curve: ParamCurve::Exponential,
        default: 12.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_DIFFUSION,
        name: "Diffuse",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.72,
    },
    ParamDescriptor {
        id: REVERB_PARAM_WIDTH,
        name: "Width",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: REVERB_PARAM_MODULATION,
        name: "Mod",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.3,
    },
    // Filters the input, not the feedback loop. A highpass inside the loop
    // compounds once per trip and would strip the bass out of the tail
    // entirely; one pass on the way in is what keeps a long hall from
    // turning to mud. See `docs/REVERB.md`.
    ParamDescriptor {
        id: REVERB_PARAM_LOW_CUT_HZ,
        name: "Low Cut",
        unit: "Hz",
        min: 20.0,
        max: 500.0,
        curve: ParamCurve::Exponential,
        default: 42.0,
    },
];

/// Parameters for the feedback-delay-network hall reverb.
///
/// Every field is a direct control of the running network: there is no
/// impulse response and no prepared resource, so a change applies on the
/// realtime side at its event offset like any other effect. That is what
/// makes the device a legal modulation destination — the convolution reverb
/// this replaced could only rebuild an IR off-thread, so a route pointed at
/// it did nothing at all.
///
/// `size` scales the network's delay lengths together; `decay_s` is the
/// mid-band RT60 the per-line feedback gains are solved for. Damping shortens
/// the high end relative to that, as a room does, so a heavily damped tail
/// measures shorter than `decay_s` on purpose.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReverbParams {
    #[serde(default = "default_reverb_size")]
    pub size: f32,
    #[serde(default = "default_reverb_decay_s")]
    pub decay_s: f32,
    #[serde(default = "default_reverb_damping")]
    pub damping: f32,
    #[serde(default = "default_reverb_predelay_ms")]
    pub predelay_ms: f32,
    #[serde(default = "default_reverb_diffusion")]
    pub diffusion: f32,
    #[serde(default = "default_reverb_width")]
    pub width: f32,
    #[serde(default = "default_reverb_modulation")]
    pub modulation: f32,
    #[serde(default = "default_reverb_low_cut_hz")]
    pub low_cut_hz: f32,
}

// Field defaults are named functions rather than a `#[serde(default)]` on the
// struct because a v1 manifest written for the generated-room reverb carries
// `decay_s` and nothing else this device understands: the remaining fields
// have to fill in individually so that one real value survives the migration.
// See `docs/PROJECT_FORMAT.md`.
const fn default_reverb_size() -> f32 {
    0.5
}
const fn default_reverb_decay_s() -> f32 {
    2.4
}
const fn default_reverb_damping() -> f32 {
    0.38
}
const fn default_reverb_predelay_ms() -> f32 {
    12.0
}
const fn default_reverb_diffusion() -> f32 {
    0.72
}
const fn default_reverb_width() -> f32 {
    1.0
}
const fn default_reverb_modulation() -> f32 {
    0.3
}
const fn default_reverb_low_cut_hz() -> f32 {
    42.0
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            size: default_reverb_size(),
            decay_s: default_reverb_decay_s(),
            damping: default_reverb_damping(),
            predelay_ms: default_reverb_predelay_ms(),
            diffusion: default_reverb_diffusion(),
            width: default_reverb_width(),
            modulation: default_reverb_modulation(),
            low_cut_hz: default_reverb_low_cut_hz(),
        }
    }
}

// --- Plate -------------------------------------------------------------

/// `Event::ParamValue` ids for [`PlateParams`].
pub const PLATE_PARAM_SIZE: u32 = 0;
pub const PLATE_PARAM_DECAY_S: u32 = 1;
pub const PLATE_PARAM_DAMPING: u32 = 2;
pub const PLATE_PARAM_WIDTH: u32 = 3;
/// Delays entry into the plate network. Appended so existing automation and
/// modulation routes retain their meanings.
pub const PLATE_PARAM_PREDELAY_MS: u32 = 4;

static PLATE_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: PLATE_PARAM_SIZE,
        name: "Size",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.5,
    },
    ParamDescriptor {
        id: PLATE_PARAM_DECAY_S,
        name: "Decay",
        unit: "s",
        min: 0.2,
        max: 10.0,
        curve: ParamCurve::Exponential,
        default: 2.0,
    },
    ParamDescriptor {
        id: PLATE_PARAM_DAMPING,
        name: "Damp",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.4,
    },
    ParamDescriptor {
        id: PLATE_PARAM_WIDTH,
        name: "Width",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    // A zero default keeps old projects bit-identical. This must stay linear:
    // the common exponential mapping requires a positive minimum.
    ParamDescriptor {
        id: PLATE_PARAM_PREDELAY_MS,
        name: "Pre",
        unit: "ms",
        min: 0.0,
        max: 200.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Parameters for the lightweight comb/allpass ("plate") reverb. A small,
/// dense, coloured box next to [`ReverbParams`]'s feedback delay network:
/// both are fixed-cost per sample, but the plate's parallel combs give it a
/// tighter, more resonant character than the FDN hall's diffuse one.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlateParams {
    pub size: f32,
    pub decay_s: f32,
    pub damping: f32,
    pub width: f32,
    #[serde(default = "default_plate_predelay_ms")]
    pub predelay_ms: f32,
}

const fn default_plate_predelay_ms() -> f32 {
    0.0
}

impl Default for PlateParams {
    fn default() -> Self {
        Self {
            size: 0.5,
            decay_s: 2.0,
            damping: 0.4,
            width: 1.0,
            predelay_ms: default_plate_predelay_ms(),
        }
    }
}

// --- Dynamics --------------------------------------------------------------
//
// Gate, compressor, and limiter share the detector and gain computers in
// `mooloop_dsp::dynamics`. They are separate kinds rather than one device
// with a mode, because their controls barely overlap and a mode switch that
// swaps every knob is not a device face.

/// `Event::ParamValue` ids for [`GateParams`].
pub const GATE_PARAM_THRESHOLD_DB: u32 = 0;
pub const GATE_PARAM_ATTACK_MS: u32 = 1;
pub const GATE_PARAM_HOLD_MS: u32 = 2;
pub const GATE_PARAM_RELEASE_MS: u32 = 3;
pub const GATE_PARAM_RANGE_DB: u32 = 4;

static GATE_DESCRIPTORS: [ParamDescriptor; 5] = [
    ParamDescriptor {
        id: GATE_PARAM_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -80.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -40.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_ATTACK_MS,
        name: "Attack",
        unit: "ms",
        min: 0.05,
        max: 100.0,
        curve: ParamCurve::Exponential,
        default: 1.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_HOLD_MS,
        name: "Hold",
        unit: "ms",
        min: 0.0,
        max: 500.0,
        curve: ParamCurve::Linear,
        default: 10.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 1.0,
        max: 2_000.0,
        curve: ParamCurve::Exponential,
        default: 100.0,
    },
    ParamDescriptor {
        id: GATE_PARAM_RANGE_DB,
        name: "Range",
        unit: "dB",
        min: -80.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -80.0,
    },
];

/// Parameters for the gate effect (`GateEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GateParams {
    pub threshold_db: f32,
    pub attack_ms: f32,
    /// How long the gate stays open after the level falls back below the
    /// threshold. Stops it chattering on material that hovers at the line.
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Attenuation applied while shut. 0 dB makes the gate inaudible.
    pub range_db: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            attack_ms: 1.0,
            hold_ms: 10.0,
            release_ms: 100.0,
            range_db: -80.0,
        }
    }
}

/// `Event::ParamValue` ids for [`CompressorParams`].
pub const COMP_PARAM_THRESHOLD_DB: u32 = 0;
pub const COMP_PARAM_RATIO: u32 = 1;
pub const COMP_PARAM_ATTACK_MS: u32 = 2;
pub const COMP_PARAM_RELEASE_MS: u32 = 3;
pub const COMP_PARAM_KNEE_DB: u32 = 4;
pub const COMP_PARAM_MAKEUP_DB: u32 = 5;
/// The compressor's own parallel balance (MOO-142): 0 is the input exactly,
/// 1 the compressed signal alone, and in between a linear blend of the two,
/// the way the channel strip's `w/d mix` is -- not the device host's
/// equal-power Wet, which runs two correlated signals 3 dB hot at 50%.
pub const COMP_PARAM_MIX: u32 = 6;

static COMPRESSOR_DESCRIPTORS: [ParamDescriptor; 7] = [
    ParamDescriptor {
        id: COMP_PARAM_THRESHOLD_DB,
        name: "Thresh",
        unit: "dB",
        min: -60.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -18.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_RATIO,
        name: "Ratio",
        unit: ":1",
        min: 1.0,
        max: 20.0,
        curve: ParamCurve::Exponential,
        default: 4.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_ATTACK_MS,
        name: "Attack",
        unit: "ms",
        min: 0.05,
        max: 200.0,
        curve: ParamCurve::Exponential,
        default: 10.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 5.0,
        max: 2_000.0,
        curve: ParamCurve::Exponential,
        default: 120.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_KNEE_DB,
        name: "Knee",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 6.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_MAKEUP_DB,
        name: "Makeup",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
    ParamDescriptor {
        id: COMP_PARAM_MIX,
        name: "Mix",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
];

/// Parameters for the compressor effect (`CompressorEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Width of the soft knee around the threshold. 0 is a hard corner.
    pub knee_db: f32,
    pub makeup_db: f32,
    /// Parallel balance; see [`COMP_PARAM_MIX`]. A song saved before it
    /// existed loads fully compressed, which is what it always was.
    #[serde(default = "default_compressor_mix")]
    pub mix: f32,
}

fn default_compressor_mix() -> f32 {
    1.0
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            knee_db: 6.0,
            makeup_db: 0.0,
            mix: default_compressor_mix(),
        }
    }
}

/// `Event::ParamValue` ids for [`LimiterParams`].
pub const LIMITER_PARAM_CEILING_DB: u32 = 0;
pub const LIMITER_PARAM_RELEASE_MS: u32 = 1;
pub const LIMITER_PARAM_GAIN_DB: u32 = 2;

static LIMITER_DESCRIPTORS: [ParamDescriptor; 3] = [
    ParamDescriptor {
        id: LIMITER_PARAM_CEILING_DB,
        name: "Ceiling",
        unit: "dB",
        min: -24.0,
        max: 0.0,
        curve: ParamCurve::Linear,
        default: -0.3,
    },
    ParamDescriptor {
        id: LIMITER_PARAM_RELEASE_MS,
        name: "Release",
        unit: "ms",
        min: 1.0,
        max: 500.0,
        curve: ParamCurve::Exponential,
        default: 50.0,
    },
    ParamDescriptor {
        id: LIMITER_PARAM_GAIN_DB,
        name: "Gain",
        unit: "dB",
        min: 0.0,
        max: 24.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

/// Parameters for the limiter effect (`LimiterEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LimiterParams {
    pub ceiling_db: f32,
    pub release_ms: f32,
    /// Input gain driven into the ceiling: this is the loudness control.
    pub gain_db: f32,
}
/// Persistent state of the retained-audio insert.
///
/// **The device is three momentary gestures over a rolling ring, plus a
/// playhead you can draw.** Each gesture computes its own start and end index
/// when it fires and owns its own settings; none of them borrows another
/// control's. That is a deliberate retreat from the turntable model this
/// device carried until 2026-09-16, in which one read head was fought over by
/// `Position`, `Rate` and `Loop` under an arbitration rule, and the buttons
/// were macros writing those shared knobs. Adam, having played it: *"i dont
/// understand what is difficult. its a buffer."*
///
/// [`Self::bars`] needs the node constructing and structurally replacing off
/// the audio thread, which is why it is not a descriptor-addressed parameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "BufferParamsOnDisk")]
pub struct BufferParams {
    #[serde(default = "default_buffer_bars")]
    pub bars: u8,
    /// Whether the writer is stopped and the ring is a sample rather than a
    /// moving window. Nonzero is frozen; the device applies hysteresis around
    /// the midpoint so a modulator resting near the threshold cannot chatter
    /// it.
    ///
    /// Frozen with nothing else driving the head, the ring **plays** -- round
    /// and round, forward, at unity. That is what makes the retained history
    /// a loop rather than a still: Adam, asked what a freeze should leave you
    /// holding, *"the frozen audio should be a musically loopable chunk"*.
    /// Quantizing the freeze to a bar line is what makes the chunk join up.
    ///
    /// It persists like every other parameter, and a project saved frozen
    /// reopens frozen -- over an **empty ring**, because the frozen audio
    /// itself is not saved yet.
    #[serde(default)]
    pub freeze: f32,
    /// Held while the JUMP gesture is down. A **gate**, not a trigger: the
    /// gesture lasts exactly as long as this is nonzero, so a lane, a MIDI
    /// note and a finger on the button all say the same thing the same way.
    ///
    /// Press freezes the ring, puts the head [`Self::jump_back`] behind the
    /// write position and plays forward from there, wrapping the ring. Release
    /// restarts the writer and returns to live.
    #[serde(default)]
    pub jump: f32,
    /// How far back JUMP lands, as a [`crate::ModTimeDivision`] index.
    #[serde(default = "default_buffer_jump_back")]
    pub jump_back: f32,
    /// Held while the REVERSE gesture is down. Press freezes the ring and
    /// plays backward from the write position, wrapping. It takes no distance:
    /// backward from *here* is the whole of what it means.
    #[serde(default)]
    pub reverse: f32,
    /// Held while the STUTTER gesture is down. Press freezes the ring and
    /// loops the last [`Self::stutter_length`] of it.
    ///
    /// It is JUMP with a repeat, which is why the two differ in one field.
    #[serde(default)]
    pub stutter: f32,
    /// How long STUTTER's repeat is, as a [`crate::ModTimeDivision`] index.
    ///
    /// **Its own setting, not a shared one.** It was `Length` -- the same knob
    /// the loop window used -- until 2026-09-16, and the question that ended
    /// that was Adam's: *"how does one set the stutter length?"* Nothing on
    /// the face was it, because the gesture overrode the knob on press and put
    /// it back on release.
    #[serde(default = "default_buffer_stutter_length")]
    pub stutter_length: f32,
    /// Where in retained memory the playhead is, normalized over
    /// [`Self::position_span`]: `0` is the old end of the span and `1` is the
    /// new end, so a rising ramp is forward playback.
    ///
    /// **It is heard only while it is moving.** A static Position is not a
    /// position, it is a setting nobody is playing, and the device falls
    /// through to live audio -- which is what makes one automatable id mean
    /// one thing whether or not anybody is driving it. Adam: *"this is the
    /// kind of thing you'd put a 1 measure sawtooth lfo on via modulator rack
    /// and it would loop through the buffer... if it is moving, thats what we
    /// should hear, otherwise its just live audio."*
    ///
    /// The head **is** the position rather than chasing it, so the playback
    /// rate is the position's own speed and needs no control: a one-bar saw
    /// over a one-bar span plays at unity, a half-bar saw plays it up an
    /// octave, and a descending ramp plays it backwards. The chase this
    /// replaced needed a time constant, an arrival test and a stillness test
    /// to decide when an edit was over, and all three are gone with it.
    #[serde(default = "default_buffer_position")]
    pub position: f32,
    /// How much of the ring [`Self::position`] spans, as a
    /// [`crate::ModTimeDivision`] index, or [`BUFFER_SPAN_FULL`] for all of
    /// it -- which is the default, because the freeze length is the obvious
    /// thing for a playhead to run over until somebody says otherwise.
    ///
    /// The span ends at the write position and extends backward, so it is
    /// always the *most recent* that much; frozen, the write position is
    /// static and the span is a fixed region of the sample.
    #[serde(default = "default_buffer_position_span")]
    pub position_span: f32,
    /// Whether a gesture and a freeze wait for the next musical boundary
    /// before they land. Nonzero is on, and **on is the default**.
    ///
    /// It buys playability at the price of latency: up to one
    /// [`Self::quant_start`] between the finger and the sound, which is the
    /// usual trade on a beat repeat and is what lets a sloppy press still
    /// land in time. Releases are never quantized -- a gesture ends when the
    /// hand says so.
    #[serde(default = "default_buffer_quantize")]
    pub quantize: f32,
    /// Which boundary [`Self::quantize`] waits for, as a
    /// [`crate::ModTimeDivision`] index.
    ///
    /// Independent of every length on the device, which is the whole of what
    /// it is for: Adam, on quantizing gestures, *"yes but you can set start
    /// and length independently"*. Starting on the quarter while stuttering a
    /// thirty-second is the ordinary case, not an exotic one.
    #[serde(default = "default_buffer_quant_start")]
    pub quant_start: f32,
    /// Declick length in milliseconds, applied wherever the head moves or the
    /// device hands back to live.
    #[serde(default = "default_buffer_crossfade_ms")]
    pub crossfade_ms: f32,
}

/// How much history a fresh Buffer keeps, in bars.
///
/// **Two, not eight.** `Position` is normalized over the ring, so the ring's
/// length is the playhead's resolution: eight bars put 50% four bars ago and
/// made every small move a leap. Two bars is the loop somebody is playing
/// over, and it is what `bars` being adjustable is for -- a pad that wants
/// eight can still say so, and now has a control that says it.
const fn default_buffer_bars() -> u8 {
    2
}

const fn default_buffer_crossfade_ms() -> f32 {
    2.5
}

/// Live: the playhead rests at the new end of its span, which is also where a
/// span of zero length would put it.
const fn default_buffer_position() -> f32 {
    1.0
}

/// The whole ring. See [`BUFFER_SPAN_FULL`].
const fn default_buffer_position_span() -> f32 {
    BUFFER_SPAN_FULL
}

/// One beat back. A jump wants to be short enough to be a stumble rather than
/// a section change, and long enough to carry a whole drum figure.
const fn default_buffer_jump_back() -> f32 {
    7.0
}

/// A sixteenth, which is what a stutter is before anybody adjusts it.
const fn default_buffer_stutter_length() -> f32 {
    13.0
}

const fn default_buffer_quantize() -> f32 {
    1.0
}

/// One bar. A gesture and the freeze that holds it land on the same boundary
/// until somebody says otherwise.
const fn default_buffer_quant_start() -> f32 {
    2.0
}

/// [`BufferParams::position_span`]'s "the whole ring" position.
///
/// One past the top of the grid, so the parameter is `ModTimeDivision::ALL`
/// plus one value rather than two settings behind one id: every position on
/// it answers the same question, *how much of the buffer does the playhead
/// run over*, and the top answer is "all of it". The range is derived from
/// the grid's own length, so a twenty-second division moves this with it.
pub const BUFFER_SPAN_FULL: f32 = crate::ModTimeDivision::ALL.len() as f32;

/// [`BufferParams`] as documents on disk spell it.
///
/// Every field is optional and defaulted, because this struct has now
/// outlived two control models. Projects written before 2026-09-16 hold
/// `offset_beats` (beats behind the writer); ones written that day hold
/// `position` (normalized, pointing the other way) plus `rate`, `length` and
/// `looping` from the turntable model. `offset_beats` still converts, because
/// the coordinate is recoverable; the other three are **dropped**, because
/// nothing in the device they configured survives to be given their value.
///
/// Serialization stays derived, so the retired keys are read and never
/// written: a song opened and saved leaves them behind for good.
#[derive(serde::Deserialize)]
struct BufferParamsOnDisk {
    #[serde(default = "default_buffer_bars")]
    bars: u8,
    #[serde(default)]
    freeze: f32,
    #[serde(default)]
    jump: f32,
    #[serde(default = "default_buffer_jump_back")]
    jump_back: f32,
    #[serde(default)]
    reverse: f32,
    #[serde(default)]
    stutter: f32,
    #[serde(default = "default_buffer_stutter_length")]
    stutter_length: f32,
    #[serde(default)]
    position: Option<f32>,
    #[serde(default)]
    position_span: Option<f32>,
    #[serde(default)]
    offset_beats: Option<f32>,
    #[serde(default = "default_buffer_quantize")]
    quantize: f32,
    /// The 2026-09-16 spelling of [`BufferParams::quant_start`]. Renamed
    /// rather than retired: it is the same number answering the same
    /// question, and it now governs a gesture's start as well as a freeze's,
    /// which is a widening rather than a change of meaning.
    #[serde(default = "default_buffer_quant_start", alias = "quant_grid")]
    quant_start: f32,
    #[serde(default = "default_buffer_crossfade_ms")]
    crossfade_ms: f32,
}

impl From<BufferParamsOnDisk> for BufferParams {
    fn from(disk: BufferParamsOnDisk) -> Self {
        let bars = disk.bars.max(1);
        // `pos = 1 - beats / history_beats`, so a head sitting at the oldest
        // sample of an eight-bar ring comes back at 0 and one sitting on the
        // writer comes back at 1. A document holding both keys is a document
        // this build wrote, so the new one wins.
        let position = disk.position.unwrap_or_else(|| {
            disk.offset_beats
                .map(|beats| {
                    let history_beats = f32::from(bars) * crate::time::BEATS_PER_BAR as f32;
                    1.0 - beats / history_beats
                })
                .unwrap_or_else(default_buffer_position)
        });
        Self {
            bars,
            freeze: disk.freeze,
            jump: disk.jump,
            jump_back: disk.jump_back,
            reverse: disk.reverse,
            stutter: disk.stutter,
            stutter_length: disk.stutter_length,
            position: position.clamp(0.0, 1.0),
            // A document from the turntable model has no span to restore, and
            // the whole ring is the reading that changes least about what it
            // sounded like.
            position_span: disk.position_span.unwrap_or_else(default_buffer_position_span),
            quantize: disk.quantize,
            quant_start: disk.quant_start,
            crossfade_ms: disk.crossfade_ms,
        }
    }
}

impl Default for BufferParams {
    fn default() -> Self {
        Self {
            bars: default_buffer_bars(),
            freeze: 0.0,
            jump: 0.0,
            jump_back: default_buffer_jump_back(),
            reverse: 0.0,
            stutter: 0.0,
            stutter_length: default_buffer_stutter_length(),
            position: default_buffer_position(),
            position_span: default_buffer_position_span(),
            quantize: default_buffer_quantize(),
            quant_start: default_buffer_quant_start(),
            crossfade_ms: default_buffer_crossfade_ms(),
        }
    }
}

/// `Event::ParamValue` ids for [`BufferParams`].
///
/// `bars` is deliberately absent. Resizing the ring reallocates, which the
/// engine does off-thread through a prepared replacement; a control-rate
/// parameter cannot do that, and pretending otherwise would put an allocation
/// on the audio thread the first time someone drew a curve on it.
///
/// **Four ids are retired and none of them may ever be reused**, because a
/// project saved before the change still names them and still means what they
/// meant then. `Offset` went on 2026-09-16 when `Position` replaced beats-
/// behind-the-writer with a normalized coordinate. `Rate`, `Length` and `Loop`
/// went later the same day with the turntable model itself: one read head
/// fought over by three standing knobs under an arbitration rule, with the
/// face's buttons as macros writing them. What replaced it is three gestures
/// that each own their settings, and a playhead that is heard while it moves.
/// None of the three has a value to inherit from a knob that no longer
/// describes anything.
pub const BUFFER_PARAM_OFFSET_BEATS: u32 = 0;
pub const BUFFER_PARAM_CROSSFADE_MS: u32 = 1;
pub const BUFFER_PARAM_POSITION: u32 = 2;
/// Retired 2026-09-16 with the turntable model. Spent.
pub const BUFFER_PARAM_RATE: u32 = 3;
/// Retired 2026-09-16 with the turntable model. Spent. The stutter's length
/// is [`BUFFER_PARAM_STUTTER_LENGTH`], which is its own setting rather than
/// this one borrowed.
pub const BUFFER_PARAM_LENGTH: u32 = 4;
/// Retired 2026-09-16 with the turntable model. Spent.
pub const BUFFER_PARAM_LOOP: u32 = 5;
pub const BUFFER_PARAM_FREEZE: u32 = 6;
pub const BUFFER_PARAM_JUMP: u32 = 7;
pub const BUFFER_PARAM_QUANTIZE: u32 = 8;
pub const BUFFER_PARAM_QUANT_START: u32 = 9;
pub const BUFFER_PARAM_JUMP_BACK: u32 = 10;
pub const BUFFER_PARAM_REVERSE: u32 = 11;
pub const BUFFER_PARAM_STUTTER: u32 = 12;
pub const BUFFER_PARAM_STUTTER_LENGTH: u32 = 13;
pub const BUFFER_PARAM_POSITION_SPAN: u32 = 14;

static BUFFER_DESCRIPTORS: [ParamDescriptor; 11] = [
    ParamDescriptor {
        id: BUFFER_PARAM_POSITION,
        name: "Position",
        unit: "%",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 1.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_POSITION_SPAN,
        name: "Span",
        unit: "",
        min: 0.0,
        max: BUFFER_SPAN_FULL,
        // One step per division plus the "whole ring" position on the end.
        curve: ParamCurve::Stepped(crate::ModTimeDivision::ALL.len() as u16 + 1),
        default: BUFFER_SPAN_FULL,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_JUMP_BACK,
        name: "Jump Back",
        unit: "",
        min: 0.0,
        max: MOD_TIME_DIVISION_TOP,
        curve: ParamCurve::Stepped(crate::ModTimeDivision::ALL.len() as u16),
        default: 7.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_STUTTER_LENGTH,
        name: "Stutter",
        unit: "",
        min: 0.0,
        max: MOD_TIME_DIVISION_TOP,
        curve: ParamCurve::Stepped(crate::ModTimeDivision::ALL.len() as u16),
        default: 13.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_CROSSFADE_MS,
        name: "Crossfade",
        unit: "ms",
        min: 0.5,
        max: 50.0,
        curve: ParamCurve::Exponential,
        default: 2.5,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_FREEZE,
        name: "Freeze",
        unit: "",
        curve: ParamCurve::Stepped(2),
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    // The three gestures are **gates**, not triggers: the sound lasts exactly
    // as long as the value is high, so a lane drawing a block, a MIDI note
    // held down and a finger on the button are one mechanism rather than
    // three. A trigger would need a second id to say when to stop.
    ParamDescriptor {
        id: BUFFER_PARAM_JUMP,
        name: "Jump",
        unit: "",
        curve: ParamCurve::Stepped(2),
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_REVERSE,
        name: "Reverse",
        unit: "",
        curve: ParamCurve::Stepped(2),
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_STUTTER,
        name: "Stutter Gate",
        unit: "",
        curve: ParamCurve::Stepped(2),
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_QUANTIZE,
        name: "Quantize",
        unit: "",
        min: 0.0,
        max: 1.0,
        curve: ParamCurve::Stepped(2),
        default: 1.0,
    },
    ParamDescriptor {
        id: BUFFER_PARAM_QUANT_START,
        name: "Quant Start",
        unit: "",
        min: 0.0,
        max: MOD_TIME_DIVISION_TOP,
        curve: ParamCurve::Stepped(crate::ModTimeDivision::ALL.len() as u16),
        default: 2.0,
    },
];

/// The top index of the shared musical grid, as a parameter range.
///
/// Derived rather than spelled, because `ModTimeDivision::ALL` is the grid and
/// a descriptor that said `20.0` would be a second copy of its length --
/// exactly the shape `scripts/dupe-audit` exists to find.
///
/// Public, because it is also the divisor every caller needs to turn a
/// normalized stepped parameter back into a grid index, and three of them had
/// written `20` by hand: two in `buffer-device.slint` and one in the Buffer's
/// telemetry. `Divisions.top` in `controls.slint` is the markup's single copy
/// and `the_slint_division_table_matches_mod_time_division` now guards it.
pub const MOD_TIME_DIVISION_TOP: f32 = crate::ModTimeDivision::ALL.len() as f32 - 1.0;

/// A container's own state, whichever kind of container it is.
///
/// **One struct for both `Chain` and `Layer`**, and it was called
/// `ChainParams` until 2026-09-21. The two kinds differ in what they *do*
/// with the rows they hold -- a chain runs them in order, a layer splits
/// across them and sums -- and not at all in what they store: a child count
/// and a mix, meaning the same two things. Naming it after one of its two
/// users is how a second copy gets written the day the other one needs a
/// field, so it is named after what it is.
///
/// The Rust name is not on the wire. `EffectParams` is a tagged enum with the
/// payload in `state`, so a chain is `{"type":"chain","state":{...}}` and a
/// layer is `{"type":"layer","state":{...}}`; the struct's identifier appears
/// in neither, and the rename cost no compatibility.
///
/// **A container does not hold its children.** `children` says how many of the
/// rows *after* this one are inside it, and those rows live in the same
/// `Vec<EffectSlotState>` as this one does. The tree is derived from the flat
/// list, exactly as a device's rack position is derived from its identity.
///
/// That is what keeps `EffectParams`, [`EffectSlotState`] and
/// `EngineCommand` `Copy` -- the command ring is preallocated POD and a `Vec`
/// in here would break all three at once -- and it is what makes "the rack
/// cannot tell the difference between a container and a leaf device"
/// structurally true rather than an invariant somebody has to maintain.
/// `docs/plans/containers/02-the-container-is-a-device.md` states the two
/// invariants the representation has to hold.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContainerParams {
    /// How many of the rows following this one are inside it.
    #[serde(default)]
    pub children: u8,
    /// Dry/wet across the whole run, applied when the run closes -- see
    /// `EffectChain::close_run`. The dry copy is taken as the box opens and
    /// delayed by the run's declared latency, so the blend is against what
    /// went in rather than against what went in `run_latency` frames ago.
    #[serde(default = "default_container_mix")]
    pub mix: f32,
}

fn default_container_mix() -> f32 {
    1.0
}

impl Default for ContainerParams {
    fn default() -> Self {
        Self {
            children: 0,
            mix: 1.0,
        }
    }
}

/// A container and everything inside it: what a container preset is.
///
/// The rows are in rack order with the container first, exactly as they sit
/// on the chain, and they carry no identities -- the ids are stripped on the
/// way out and minted fresh on the way in, because identity belongs to the
/// chain a device is on rather than to the patch. See
/// `docs/plans/containers/05-a-container-is-a-preset.md`.
///
/// **The modulation that drives the run is deliberately not here.** A route's
/// source is a module in the *channel's* rack, not in the container, so a run
/// saved with its routes would load onto a channel whose rack has no such
/// module. Carrying the modules too would mean deciding that a modulator can
/// live in a container, which `reference/CONTAINERS.md` records as Adam's
/// undecided call and explicitly does not block this work. The field this
/// would grow is the reason `contains` is a list.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectRun {
    pub effects: Vec<EffectSlotState>,
}

/// `Event::ParamValue` ids for [`ContainerParams`].
///
/// `children` is deliberately absent, for the same reason `BufferParams.bars`
/// is: it is structure rather than a control. A curve drawn on it would
/// rewrite the shape of the chain from the audio thread.
pub const CONTAINER_PARAM_MIX: u32 = 0;

static CONTAINER_DESCRIPTORS: [ParamDescriptor; 1] = [ParamDescriptor {
    id: CONTAINER_PARAM_MIX,
    name: "Mix",
    unit: "",
    min: 0.0,
    max: 1.0,
    curve: ParamCurve::Linear,
    default: 1.0,
}];

impl Default for LimiterParams {
    fn default() -> Self {
        Self {
            ceiling_db: -0.3,
            release_ms: 50.0,
            gain_db: 0.0,
        }
    }
}

// --- Slot state ------------------------------------------------------------

/// Per-kind parameter set. Tagged with the same serde shape as
/// `ChannelSource` so new kinds join the v1 envelope additively.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum EffectParams {
    Eq(EqParams),
    Modulation(ModulationParams),
    Filter(FilterParams),
    Drive(DriveParams),
    Preamp(PreampParams),
    Bitcrush(BitcrushParams),
    Delay(DelayParams),
    Reverb(ReverbParams),
    Plate(PlateParams),
    Gate(GateParams),
    Compressor(CompressorParams),
    Limiter(LimiterParams),
    Buffer(BufferParams),
    Chain(ContainerParams),
    Layer(ContainerParams),
}

impl EffectParams {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Eq(_) => EffectKind::Eq,
            Self::Modulation(_) => EffectKind::Modulation,
            Self::Filter(_) => EffectKind::Filter,
            Self::Drive(_) => EffectKind::Drive,
            Self::Preamp(_) => EffectKind::Preamp,
            Self::Bitcrush(_) => EffectKind::Bitcrush,
            Self::Delay(_) => EffectKind::Delay,
            Self::Reverb(_) => EffectKind::Reverb,
            Self::Plate(_) => EffectKind::Plate,
            Self::Gate(_) => EffectKind::Gate,
            Self::Compressor(_) => EffectKind::Compressor,
            Self::Limiter(_) => EffectKind::Limiter,
            Self::Buffer(_) => EffectKind::Buffer,
            Self::Chain(_) => EffectKind::Chain,
            Self::Layer(_) => EffectKind::Layer,
        }
    }

    /// How many of the rows after this one are inside it, or `None` for a
    /// device that holds nothing.
    ///
    /// **The one place the workspace asks "is this a container, and how big
    /// is it".** It was spelled `EffectParams::Chain(chain)` at thirty-three
    /// sites across seven crates — thirteen of them in `structure.rs`, which
    /// is where the span primitives live — and a value written down that many
    /// times is this codebase's characteristic fault waiting for a second
    /// writer. The second writer is `EffectKind::Layer`
    /// (`docs/plans/containers/07-a-branch-is-a-run.md`): every one of those
    /// sites would have had to grow an arm, and the ones that did not would
    /// have gone on quietly treating a layer as an ordinary device whose
    /// children are loose rows of the chain.
    ///
    /// So the span primitives read this and **none of them names a container
    /// kind at all**. What a container *does* with its rows — run them in
    /// order, or split across them — is a separate question, asked in the two
    /// places that render and draw, and it does not belong here.
    pub fn container_children(&self) -> Option<u8> {
        match self {
            Self::Chain(container) | Self::Layer(container) => Some(container.children),
            _ => None,
        }
    }

    /// Whether this device holds a run of the rows after it.
    pub fn is_container(&self) -> bool {
        self.container_children().is_some()
    }

    /// How this device runs the rows it holds, or `None` for a device that
    /// holds nothing. [`EffectKind::container_flow`], asked of a value.
    pub fn container_flow(&self) -> Option<ContainerFlow> {
        self.kind().container_flow()
    }

    /// Set how many of the rows after this one are inside it.
    ///
    /// Does nothing to a device that holds nothing, which is what every call
    /// site already meant: each was an `if let` whose `else` branch was the
    /// empty statement. Saturation and clamping stay with the caller, because
    /// they differ — a resize clamps a signed delta into range, an insert
    /// saturates, and a wrap refuses outright rather than truncating a run it
    /// cannot describe.
    pub fn set_container_children(&mut self, children: u8) {
        if let Self::Chain(container) | Self::Layer(container) = self {
            container.children = children;
        }
    }

    pub fn filter(&self) -> Option<&FilterParams> {
        match self {
            Self::Filter(p) => Some(p),
            _ => None,
        }
    }

    pub fn eq(&self) -> Option<&EqParams> {
        match self {
            Self::Eq(p) => Some(p),
            _ => None,
        }
    }

    pub fn eq_mut(&mut self) -> Option<&mut EqParams> {
        match self {
            Self::Eq(p) => Some(p),
            _ => None,
        }
    }

    pub fn modulation(&self) -> Option<&ModulationParams> {
        match self {
            Self::Modulation(p) => Some(p),
            _ => None,
        }
    }

    pub fn preamp(&self) -> Option<&PreampParams> {
        match self {
            Self::Preamp(p) => Some(p),
            _ => None,
        }
    }

    pub fn drive(&self) -> Option<&DriveParams> {
        match self {
            Self::Drive(p) => Some(p),
            _ => None,
        }
    }

    pub fn bitcrush(&self) -> Option<&BitcrushParams> {
        match self {
            Self::Bitcrush(p) => Some(p),
            _ => None,
        }
    }

    pub fn delay(&self) -> Option<&DelayParams> {
        match self {
            Self::Delay(p) => Some(p),
            _ => None,
        }
    }

    pub fn reverb(&self) -> Option<&ReverbParams> {
        match self {
            Self::Reverb(p) => Some(p),
            _ => None,
        }
    }

    pub fn plate(&self) -> Option<&PlateParams> {
        match self {
            Self::Plate(p) => Some(p),
            _ => None,
        }
    }

    pub fn gate(&self) -> Option<&GateParams> {
        match self {
            Self::Gate(p) => Some(p),
            _ => None,
        }
    }

    pub fn compressor(&self) -> Option<&CompressorParams> {
        match self {
            Self::Compressor(p) => Some(p),
            _ => None,
        }
    }

    pub fn limiter(&self) -> Option<&LimiterParams> {
        match self {
            Self::Limiter(p) => Some(p),
            _ => None,
        }
    }

    pub fn buffer(&self) -> Option<&BufferParams> {
        match self {
            Self::Buffer(p) => Some(p),
            _ => None,
        }
    }

    /// Read one parameter in natural units by wire id. Returns `None` for an
    /// id this kind does not have.
    pub fn get(&self, id: u32) -> Option<f32> {
        match self {
            // Nothing here reads `selected_target`. That is the whole of
            // this device's 2026-09-14 change: an id names one field of one
            // band, so what the face happens to be showing cannot alter what
            // a lane means.
            Self::Eq(p) => {
                if let Some((band, field)) = eq_band_of(id) {
                    let band = p.bands.get(band)?;
                    return Some(match field {
                        EQ_BAND_ON => bool_to_f32(band.enabled),
                        EQ_BAND_FREQ => band.frequency_hz,
                        EQ_BAND_GAIN => band.gain_db,
                        EQ_BAND_Q => band.q,
                        EQ_BAND_KIND => band.kind.to_index() as f32,
                        _ => band.q_profile.to_index() as f32,
                    });
                }
                let (pass, field) = eq_pass_of(id)?;
                let pass = p.pass(pass)?;
                Some(match field {
                    EQ_PASS_ON => bool_to_f32(pass.enabled),
                    EQ_PASS_FREQ => pass.frequency_hz,
                    EQ_PASS_Q => pass.q,
                    _ => pass.slope.to_index() as f32,
                })
            }
            Self::Modulation(p) => match id {
                MODULATION_PARAM_MODE => Some(p.mode.to_index() as f32),
                MODULATION_PARAM_RATE_HZ => Some(p.rate_hz),
                MODULATION_PARAM_DEPTH => Some(p.depth),
                MODULATION_PARAM_COLOR => Some(p.color),
                MODULATION_PARAM_FEEDBACK => Some(p.feedback),
                MODULATION_PARAM_SPREAD => Some(p.spread),
                MODULATION_PARAM_TONE => Some(p.tone),
                MODULATION_PARAM_STAGES => Some(f32::from(p.stages)),
                _ => None,
            },
            Self::Filter(p) => match id {
                FILTER_PARAM_CUTOFF_HZ => Some(p.cutoff_hz),
                FILTER_PARAM_RESONANCE => Some(p.resonance),
                FILTER_PARAM_MODE => Some(p.mode.to_index() as f32),
                FILTER_PARAM_SLOPE => Some(p.slope.to_index() as f32),
                FILTER_PARAM_DRIVE => Some(p.drive),
                _ => None,
            },
            Self::Drive(p) => match id {
                DRIVE_PARAM_DRIVE => Some(p.drive),
                DRIVE_PARAM_CURVE => Some(p.curve.to_index() as f32),
                DRIVE_PARAM_TONE => Some(p.tone),
                DRIVE_PARAM_MIX => Some(p.mix),
                DRIVE_PARAM_OUTPUT => Some(p.output),
                _ => None,
            },
            Self::Preamp(p) => match id {
                PREAMP_PARAM_DRIVE_DB => Some(p.drive_db),
                PREAMP_PARAM_VOICING => Some(p.voicing.to_index() as f32),
                PREAMP_PARAM_MIX => Some(p.mix),
                PREAMP_PARAM_OUTPUT_DB => Some(p.output_db),
                _ => None,
            },
            Self::Bitcrush(p) => match id {
                BITCRUSH_PARAM_BITS => Some(p.bits),
                BITCRUSH_PARAM_DOWNSAMPLE => Some(p.downsample),
                BITCRUSH_PARAM_MIX => Some(p.mix),
                BITCRUSH_PARAM_STYLE => Some(p.style.to_index() as f32),
                _ => None,
            },
            Self::Delay(p) => match id {
                DELAY_PARAM_TIME_MS => Some(p.time_ms),
                DELAY_PARAM_FEEDBACK => Some(p.feedback),
                DELAY_PARAM_MODE => Some(p.mode.to_index() as f32),
                DELAY_PARAM_CROSS => Some(p.cross),
                DELAY_PARAM_TONE => Some(p.tone),
                DELAY_PARAM_MIX => Some(p.mix),
                _ => None,
            },
            Self::Reverb(p) => match id {
                REVERB_PARAM_SIZE => Some(p.size),
                REVERB_PARAM_DECAY_S => Some(p.decay_s),
                REVERB_PARAM_DAMPING => Some(p.damping),
                REVERB_PARAM_PREDELAY_MS => Some(p.predelay_ms),
                REVERB_PARAM_DIFFUSION => Some(p.diffusion),
                REVERB_PARAM_WIDTH => Some(p.width),
                REVERB_PARAM_MODULATION => Some(p.modulation),
                REVERB_PARAM_LOW_CUT_HZ => Some(p.low_cut_hz),
                _ => None,
            },
            Self::Plate(p) => match id {
                PLATE_PARAM_SIZE => Some(p.size),
                PLATE_PARAM_DECAY_S => Some(p.decay_s),
                PLATE_PARAM_DAMPING => Some(p.damping),
                PLATE_PARAM_WIDTH => Some(p.width),
                PLATE_PARAM_PREDELAY_MS => Some(p.predelay_ms),
                _ => None,
            },
            Self::Gate(p) => match id {
                GATE_PARAM_THRESHOLD_DB => Some(p.threshold_db),
                GATE_PARAM_ATTACK_MS => Some(p.attack_ms),
                GATE_PARAM_HOLD_MS => Some(p.hold_ms),
                GATE_PARAM_RELEASE_MS => Some(p.release_ms),
                GATE_PARAM_RANGE_DB => Some(p.range_db),
                _ => None,
            },
            Self::Compressor(p) => match id {
                COMP_PARAM_THRESHOLD_DB => Some(p.threshold_db),
                COMP_PARAM_RATIO => Some(p.ratio),
                COMP_PARAM_ATTACK_MS => Some(p.attack_ms),
                COMP_PARAM_RELEASE_MS => Some(p.release_ms),
                COMP_PARAM_KNEE_DB => Some(p.knee_db),
                COMP_PARAM_MAKEUP_DB => Some(p.makeup_db),
                COMP_PARAM_MIX => Some(p.mix),
                _ => None,
            },
            Self::Limiter(p) => match id {
                LIMITER_PARAM_CEILING_DB => Some(p.ceiling_db),
                LIMITER_PARAM_RELEASE_MS => Some(p.release_ms),
                LIMITER_PARAM_GAIN_DB => Some(p.gain_db),
                _ => None,
            },
            Self::Buffer(p) => match id {
                BUFFER_PARAM_POSITION => Some(p.position),
                BUFFER_PARAM_POSITION_SPAN => Some(p.position_span),
                BUFFER_PARAM_CROSSFADE_MS => Some(p.crossfade_ms),
                BUFFER_PARAM_FREEZE => Some(p.freeze),
                BUFFER_PARAM_JUMP => Some(p.jump),
                BUFFER_PARAM_JUMP_BACK => Some(p.jump_back),
                BUFFER_PARAM_REVERSE => Some(p.reverse),
                BUFFER_PARAM_STUTTER => Some(p.stutter),
                BUFFER_PARAM_STUTTER_LENGTH => Some(p.stutter_length),
                BUFFER_PARAM_QUANTIZE => Some(p.quantize),
                BUFFER_PARAM_QUANT_START => Some(p.quant_start),
                _ => None,
            },
            Self::Chain(p) | Self::Layer(p) => match id {
                CONTAINER_PARAM_MIX => Some(p.mix),
                _ => None,
            },
        }
    }

    /// Write one parameter in natural units by wire id, clamped through its
    /// descriptor. Returns the stored value, or `None` for an unknown id.
    pub fn set(&mut self, id: u32, value: f32) -> Option<f32> {
        let descriptor = self.kind().descriptor(id)?;
        let value = descriptor.clamp_natural(value);
        match self {
            Self::Eq(p) => {
                if let Some((band, field)) = eq_band_of(id) {
                    let band = p.bands.get_mut(band)?;
                    match field {
                        EQ_BAND_ON => band.enabled = value >= 0.5,
                        EQ_BAND_FREQ => band.frequency_hz = value,
                        EQ_BAND_GAIN => band.gain_db = value,
                        EQ_BAND_Q => band.q = value,
                        EQ_BAND_KIND => band.kind = EqBandKind::from_index(value.round() as i32),
                        _ => band.q_profile = EqQProfile::from_index(value.round() as i32),
                    }
                } else {
                    let (pass, field) = eq_pass_of(id)?;
                    let pass = p.pass_mut(pass)?;
                    match field {
                        EQ_PASS_ON => pass.enabled = value >= 0.5,
                        EQ_PASS_FREQ => pass.frequency_hz = value,
                        EQ_PASS_Q => pass.q = value,
                        _ => pass.slope = EqSlope::from_index(value.round() as i32),
                    }
                }
            }
            Self::Modulation(p) => match id {
                MODULATION_PARAM_MODE => p.mode = ModulationMode::from_index(value.round() as i32),
                MODULATION_PARAM_RATE_HZ => p.rate_hz = value,
                MODULATION_PARAM_DEPTH => p.depth = value,
                MODULATION_PARAM_COLOR => p.color = value,
                MODULATION_PARAM_FEEDBACK => p.feedback = value,
                MODULATION_PARAM_SPREAD => p.spread = value,
                MODULATION_PARAM_TONE => p.tone = value,
                MODULATION_PARAM_STAGES => p.stages = value.round() as u8,
                _ => return None,
            },
            Self::Filter(p) => match id {
                FILTER_PARAM_CUTOFF_HZ => p.cutoff_hz = value,
                FILTER_PARAM_RESONANCE => p.resonance = value,
                FILTER_PARAM_MODE => p.mode = FilterMode::from_index(value.round() as i32),
                FILTER_PARAM_SLOPE => p.slope = FilterSlope::from_index(value.round() as i32),
                FILTER_PARAM_DRIVE => p.drive = value,
                _ => return None,
            },
            Self::Drive(p) => match id {
                DRIVE_PARAM_DRIVE => p.drive = value,
                DRIVE_PARAM_CURVE => p.curve = DriveCurve::from_index(value.round() as i32),
                DRIVE_PARAM_TONE => p.tone = value,
                DRIVE_PARAM_MIX => p.mix = value,
                DRIVE_PARAM_OUTPUT => p.output = value,
                _ => return None,
            },
            Self::Preamp(p) => match id {
                PREAMP_PARAM_DRIVE_DB => p.drive_db = value,
                PREAMP_PARAM_VOICING => {
                    p.voicing = PreampVoicing::from_index(value.round() as i32)
                }
                PREAMP_PARAM_MIX => p.mix = value,
                PREAMP_PARAM_OUTPUT_DB => p.output_db = value,
                _ => return None,
            },
            Self::Bitcrush(p) => match id {
                BITCRUSH_PARAM_BITS => p.bits = value,
                BITCRUSH_PARAM_DOWNSAMPLE => p.downsample = value,
                BITCRUSH_PARAM_MIX => p.mix = value,
                BITCRUSH_PARAM_STYLE => p.style = BitcrushStyle::from_index(value.round() as i32),
                _ => return None,
            },
            Self::Delay(p) => match id {
                DELAY_PARAM_TIME_MS => p.time_ms = value,
                DELAY_PARAM_FEEDBACK => p.feedback = value,
                DELAY_PARAM_MODE => p.mode = DelayMode::from_index(value.round() as i32),
                DELAY_PARAM_CROSS => p.cross = value,
                DELAY_PARAM_TONE => p.tone = value,
                DELAY_PARAM_MIX => p.mix = value,
                _ => return None,
            },
            Self::Reverb(p) => match id {
                REVERB_PARAM_SIZE => p.size = value,
                REVERB_PARAM_DECAY_S => p.decay_s = value,
                REVERB_PARAM_DAMPING => p.damping = value,
                REVERB_PARAM_PREDELAY_MS => p.predelay_ms = value,
                REVERB_PARAM_DIFFUSION => p.diffusion = value,
                REVERB_PARAM_WIDTH => p.width = value,
                REVERB_PARAM_MODULATION => p.modulation = value,
                REVERB_PARAM_LOW_CUT_HZ => p.low_cut_hz = value,
                _ => return None,
            },
            Self::Plate(p) => match id {
                PLATE_PARAM_SIZE => p.size = value,
                PLATE_PARAM_DECAY_S => p.decay_s = value,
                PLATE_PARAM_DAMPING => p.damping = value,
                PLATE_PARAM_WIDTH => p.width = value,
                PLATE_PARAM_PREDELAY_MS => p.predelay_ms = value,
                _ => return None,
            },
            Self::Gate(p) => match id {
                GATE_PARAM_THRESHOLD_DB => p.threshold_db = value,
                GATE_PARAM_ATTACK_MS => p.attack_ms = value,
                GATE_PARAM_HOLD_MS => p.hold_ms = value,
                GATE_PARAM_RELEASE_MS => p.release_ms = value,
                GATE_PARAM_RANGE_DB => p.range_db = value,
                _ => return None,
            },
            Self::Compressor(p) => match id {
                COMP_PARAM_THRESHOLD_DB => p.threshold_db = value,
                COMP_PARAM_RATIO => p.ratio = value,
                COMP_PARAM_ATTACK_MS => p.attack_ms = value,
                COMP_PARAM_RELEASE_MS => p.release_ms = value,
                COMP_PARAM_KNEE_DB => p.knee_db = value,
                COMP_PARAM_MAKEUP_DB => p.makeup_db = value,
                COMP_PARAM_MIX => p.mix = value,
                _ => return None,
            },
            Self::Limiter(p) => match id {
                LIMITER_PARAM_CEILING_DB => p.ceiling_db = value,
                LIMITER_PARAM_RELEASE_MS => p.release_ms = value,
                LIMITER_PARAM_GAIN_DB => p.gain_db = value,
                _ => return None,
            },
            Self::Buffer(p) => match id {
                BUFFER_PARAM_POSITION => p.position = value,
                BUFFER_PARAM_POSITION_SPAN => p.position_span = value,
                BUFFER_PARAM_CROSSFADE_MS => p.crossfade_ms = value,
                BUFFER_PARAM_FREEZE => p.freeze = value,
                BUFFER_PARAM_JUMP => p.jump = value,
                BUFFER_PARAM_JUMP_BACK => p.jump_back = value,
                BUFFER_PARAM_REVERSE => p.reverse = value,
                BUFFER_PARAM_STUTTER => p.stutter = value,
                BUFFER_PARAM_STUTTER_LENGTH => p.stutter_length = value,
                BUFFER_PARAM_QUANTIZE => p.quantize = value,
                BUFFER_PARAM_QUANT_START => p.quant_start = value,
                _ => return None,
            },
            Self::Chain(p) | Self::Layer(p) => match id {
                CONTAINER_PARAM_MIX => p.mix = value,
                _ => return None,
            },
        }
        Some(value)
    }
}

/// Songs written before `EffectParams` was tagged stored a bare `FilterParams`
/// table, because `Filter` was the only kind. Accept both shapes on load.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum EffectParamsCompat {
    Tagged(EffectParams),
    LegacyFilter(FilterParams),
}

fn deserialize_effect_params<'de, D>(deserializer: D) -> Result<EffectParams, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Ok(match EffectParamsCompat::deserialize(deserializer)? {
        EffectParamsCompat::Tagged(params) => params,
        EffectParamsCompat::LegacyFilter(params) => EffectParams::Filter(params),
    })
}

/// A rack device's identity, stable within one chain.
///
/// Minted when the device is inserted, carried through reorders, and never
/// reused, so a route outlives a slot number. The same shape and the same
/// argument as [`crate::ModSourceId`]: routes and lanes name the id, and the
/// position is derived from the chain rather than persisted.
///
/// [`Self::UNASSIGNED`] is what a slot state carries before it has joined a
/// chain -- a factory patch, a preset on disk, a value under construction.
/// Nothing addressable ever holds it: [`crate::insert_effect`] mints a real
/// one on the way in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct DeviceId(pub u32);

impl DeviceId {
    /// Not yet part of a chain. Deliberately the top of the space rather than
    /// zero, so a value that escapes without being minted addresses nothing
    /// instead of addressing the first device.
    pub const UNASSIGNED: Self = Self(u32::MAX);

    pub const fn is_assigned(self) -> bool {
        self.0 != Self::UNASSIGNED.0
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::UNASSIGNED
    }
}

/// A channel's durable identity, for the same reason and in the same shape as
/// [`DeviceId`] one type up.
///
/// A channel today is its position in `Project.channels`, so every insert,
/// removal and move renumbers everything that named one. An id is what lets a
/// saved address, a session key and -- from `channel-identity/05` -- an engine
/// strip survive that renumbering: the position is derived from the list when
/// it is needed, through [`crate::Project::channel_index`].
///
/// Minted from `Project.next_channel_id` and never reused, so an address left
/// holding the id of a deleted channel resolves to nothing rather than to
/// whichever channel slid into its seat.
///
/// [`Self::UNASSIGNED`] is the top of the range rather than zero for
/// `DeviceId`'s reason: a value that escapes without being minted must address
/// nothing, not the first channel.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ChannelId(pub u32);

impl ChannelId {
    /// Not yet part of a song: a pasted channel, a value under construction,
    /// or a row in a document written before channels had identities.
    pub const UNASSIGNED: Self = Self(u32::MAX);

    pub const fn is_assigned(self) -> bool {
        self.0 != Self::UNASSIGNED.0
    }
}

impl Default for ChannelId {
    fn default() -> Self {
        Self::UNASSIGNED
    }
}

/// `skip_serializing_if` for every field holding a [`ChannelId`].
///
/// One predicate rather than one per module: a channel's own `id`, an Aux In's
/// `source_id` and an envelope gate's `input_channel_id` all mean the same
/// thing by being absent, and three copies of `!id.is_assigned()` is three
/// chances for one of them to start meaning something else.
pub fn channel_id_is_unassigned(id: &ChannelId) -> bool {
    !id.is_assigned()
}

/// A track's durable identity: [`ChannelId`] one list over, in the same
/// shape and for the same reason.
///
/// Addresses that name a track -- a channel's `bus`, a track's output and
/// sends, `EffectTarget::Bus` -- are still seats, renumbered by
/// [`crate::structure::TrackEdit`]. What needs the identity is the engine: an
/// install matches the tracks of the outgoing and incoming projects by it, so
/// a track's live strip survives an edit that moved it
/// (`docs/plans/archive/incremental-structure/`).
///
/// Minted from `Project.next_track_id` and never reused.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct TrackId(pub u32);

impl TrackId {
    /// Not yet part of a song: a track under construction, or one in a file
    /// written before tracks had identities.
    pub const UNASSIGNED: Self = Self(u32::MAX);

    pub const fn is_assigned(self) -> bool {
        self.0 != Self::UNASSIGNED.0
    }
}

impl Default for TrackId {
    fn default() -> Self {
        Self::UNASSIGNED
    }
}

/// `skip_serializing_if` for a [`TrackId`].
pub fn track_id_is_unassigned(id: &TrackId) -> bool {
    !id.is_assigned()
}

/// Persisted state of one slot in a channel's effect chain.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectSlotState {
    /// This device's durable identity.
    ///
    /// Absent in every project written before devices had one, and in a
    /// preset, which carries no identity of its own. A chain decoded without
    /// ids takes its positions as its ids -- see
    /// `ChannelSetup::assign_device_ids` -- which is exactly what the routes
    /// in such a project already mean by `slot`.
    #[serde(default, skip_serializing_if = "id_is_unassigned")]
    pub id: DeviceId,
    #[serde(deserialize_with = "deserialize_effect_params")]
    pub params: EffectParams,
    pub bypassed: bool,
    #[serde(default = "default_wet_dry")]
    pub wet_dry: f32,
    #[serde(default = "default_input_trim")]
    pub input_trim: f32,
    #[serde(default = "default_output_trim")]
    pub output_trim: f32,
}

fn id_is_unassigned(id: &DeviceId) -> bool {
    !id.is_assigned()
}

fn default_wet_dry() -> f32 {
    1.0
}
fn default_input_trim() -> f32 {
    1.0
}
fn default_output_trim() -> f32 {
    1.0
}

impl EffectSlotState {
    pub fn new(params: EffectParams) -> Self {
        Self {
            id: DeviceId::UNASSIGNED,
            params,
            bypassed: false,
            wet_dry: 1.0,
            input_trim: 1.0,
            output_trim: 1.0,
        }
    }

    /// A slot holding this kind's defaults.
    pub fn of_kind(kind: EffectKind) -> Self {
        let mut slot = Self::new(kind.default_params());
        // Chosen against the level-matched wet path (step 07 of the gain
        // plan): a blend is a *ratio*, and 0.35 was picked when the reverb's
        // wet output sat ~10 dB hot, making it near-full wet in practice.
        // At parity, 0.25 is a clear but background space; modulation keeps
        // 0.5 because a chorus wants equal standing to do its combing.
        if kind == EffectKind::Reverb || kind == EffectKind::Plate {
            slot.wet_dry = 0.25;
        } else if kind == EffectKind::Modulation {
            slot.wet_dry = 0.5;
        }
        slot
    }

    pub fn filter(params: FilterParams) -> Self {
        Self::new(EffectParams::Filter(params))
    }

    pub fn drive(params: DriveParams) -> Self {
        Self::new(EffectParams::Drive(params))
    }

    pub fn preamp(params: PreampParams) -> Self {
        Self::new(EffectParams::Preamp(params))
    }

    pub fn modulation(params: ModulationParams) -> Self {
        Self::new(EffectParams::Modulation(params))
    }

    pub fn bitcrush(params: BitcrushParams) -> Self {
        Self::new(EffectParams::Bitcrush(params))
    }

    pub fn delay(params: DelayParams) -> Self {
        Self::new(EffectParams::Delay(params))
    }

    pub fn gate(params: GateParams) -> Self {
        Self::new(EffectParams::Gate(params))
    }

    pub fn compressor(params: CompressorParams) -> Self {
        Self::new(EffectParams::Compressor(params))
    }

    pub fn limiter(params: LimiterParams) -> Self {
        Self::new(EffectParams::Limiter(params))
    }

    pub fn kind(&self) -> EffectKind {
        self.params.kind()
    }

    /// The same device wearing `id`.
    ///
    /// A preset arrives without an identity and must not bring one: loading
    /// one over a live row keeps the row's id, because the routes and lanes
    /// pointing at that row are pointing at the *device*, and a preset load
    /// changes what it sounds like rather than which one it is.
    pub fn with_id(mut self, id: DeviceId) -> Self {
        self.id = id;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard on the sweep that replaced thirty-three hand-written
    /// `EffectParams::Chain(_)` tests, and it is `AGENTS.md`'s question about
    /// a mirrored value: **does anything read the copy the test checks?**
    ///
    /// It does — `EffectKind::is_container` and `EffectParams::is_container`
    /// are one fact asked of a kind and of a value, and the span primitives
    /// read the second while `integrity.rs` and the renderer's install read
    /// the first. A fifteenth kind that answered them differently would put
    /// a container's rows loose in the chain on one path and inside the box
    /// on the other, with nothing failing.
    ///
    /// `EffectKind::ALL` is what makes this a sweep rather than a list
    /// somebody has to remember to extend.
    #[test]
    fn container_predicates_agree_about_every_kind() {
        for kind in EffectKind::ALL {
            let params = kind.default_params();
            assert_eq!(
                kind.is_container(),
                params.is_container(),
                "{kind:?} answers is_container two different ways"
            );
            assert_eq!(
                params.is_container(),
                params.container_children().is_some(),
                "{kind:?} is a container that cannot count its rows"
            );
            assert_eq!(
                params.is_container(),
                params.container_flow().is_some(),
                "{kind:?} is a container that cannot say how it runs its rows"
            );
        }
    }

    /// **The two container kinds differ in one answer, and it is this one.**
    ///
    /// Stated per kind rather than swept, because the sweep above can only
    /// say that every container has *a* flow. A layer that answered `Series`
    /// would pass it, run its branches one into the next, and still be called
    /// a layer -- which is how `containers/07` landed it on purpose, and
    /// what `containers/08` made untrue: the renderer and the latency walk
    /// both read this answer to decide whether to split.
    #[test]
    fn a_chain_runs_in_series_and_a_layer_in_parallel() {
        assert_eq!(EffectKind::Chain.container_flow(), Some(ContainerFlow::Series));
        assert_eq!(EffectKind::Layer.container_flow(), Some(ContainerFlow::Parallel));
        let parallel: Vec<EffectKind> = EffectKind::ALL
            .into_iter()
            .filter(|kind| kind.container_flow() == Some(ContainerFlow::Parallel))
            .collect();
        assert_eq!(parallel, [EffectKind::Layer], "only a layer splits its input");
    }

    /// A layer is a new `type` tag and nothing else: a file naming it decodes
    /// with the container defaults, and a chain written before `Layer`
    /// existed decodes exactly as it did. `PROJECT_FORMAT.md`'s additive rule
    /// at the one place a new params variant could break it.
    #[test]
    fn a_layer_is_a_new_tag_on_the_same_state() {
        let layer: EffectParams =
            toml::from_str("type = \"layer\"\n[state]\n").expect("an empty layer state decodes");
        assert_eq!(layer, EffectParams::Layer(ContainerParams::default()));

        let dialled = EffectParams::Layer(ContainerParams {
            children: 2,
            mix: 0.25,
        });
        let written = toml::to_string(&dialled).expect("a layer encodes");
        assert!(
            written.contains("type = \"layer\""),
            "a layer is written under its own tag, got {written:?}"
        );
        assert_eq!(toml::from_str::<EffectParams>(&written).expect("and reads back"), dialled);

        let chain: EffectParams =
            toml::from_str("type = \"chain\"\n[state]\nchildren = 2\nmix = 0.25\n")
                .expect("a chain still decodes");
        assert_eq!(
            chain,
            EffectParams::Chain(ContainerParams {
                children: 2,
                mix: 0.25
            })
        );
    }

    /// Each of the seven call sites this setter replaced was an `if let`
    /// whose `else` was the empty statement, so a device that holds nothing
    /// has to absorb the write rather than panic or store it somewhere.
    #[test]
    fn setting_children_does_nothing_to_a_device_that_holds_nothing() {
        for kind in EffectKind::ALL {
            let mut params = kind.default_params();
            params.set_container_children(3);
            assert_eq!(
                params.container_children(),
                kind.is_container().then_some(3),
                "{kind:?} took a child count it should not have"
            );
        }
    }

    #[test]
    fn normalization_round_trips_across_every_descriptor() {
        for kind in EffectKind::ALL {
            for descriptor in kind.descriptors() {
                for step in 0..=10 {
                    let norm = step as f32 / 10.0;
                    let natural = descriptor.from_normalized(norm);
                    assert!(
                        natural >= descriptor.min - 1e-3 && natural <= descriptor.max + 1e-3,
                        "{}/{} produced {natural} outside [{}, {}]",
                        kind.label(),
                        descriptor.name,
                        descriptor.min,
                        descriptor.max
                    );
                    let back = descriptor.to_normalized(natural);
                    let tolerance = match descriptor.curve {
                        // Stepped params snap, so only the snapped positions
                        // round-trip exactly.
                        ParamCurve::Stepped(_) => 0.5,
                        _ => 1e-3,
                    };
                    assert!(
                        (back - norm).abs() <= tolerance,
                        "{}/{} round-tripped {norm} to {back}",
                        kind.label(),
                        descriptor.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_descriptor_default_is_in_range() {
        for kind in EffectKind::ALL {
            for descriptor in kind.descriptors() {
                assert!(
                    descriptor.default >= descriptor.min && descriptor.default <= descriptor.max,
                    "{}/{} default {} outside range",
                    kind.label(),
                    descriptor.name,
                    descriptor.default
                );
            }
        }
    }

    #[test]
    fn descriptor_defaults_match_the_params_defaults() {
        for kind in EffectKind::ALL {
            let params = kind.default_params();
            for descriptor in kind.descriptors() {
                let actual = params.get(descriptor.id).unwrap_or_else(|| {
                    panic!("{}/{} has no getter", kind.label(), descriptor.name)
                });
                assert!(
                    (actual - descriptor.default).abs() <= 1e-4,
                    "{}/{}: descriptor says {}, params say {actual}",
                    kind.label(),
                    descriptor.name,
                    descriptor.default
                );
            }
        }
    }

    /// **The bank spreads, and a shelf sits at each end of it.**
    ///
    /// Three bands were on and the other four were parked at 1 kHz until
    /// 2026-09-15, so a seven-band EQ drew four of its handles underneath
    /// band 2's and the high shelf was band 3 -- a third of the way along a
    /// row of seven buttons. This is the shape of the bank a face can rely
    /// on: every band on, every band somewhere of its own, the shelves at
    /// the ends.
    #[test]
    fn the_seven_bands_rest_spread_out_with_a_shelf_at_each_end() {
        let eq = EqParams::default();
        assert_eq!(eq.bands[0].kind, EqBandKind::LowShelf);
        assert_eq!(eq.bands[EQ_MAX_BANDS - 1].kind, EqBandKind::HighShelf);
        for (index, band) in eq.bands.iter().enumerate() {
            assert!(band.enabled, "band {} starts off", index + 1);
            assert_eq!(band.gain_db, 0.0, "band {} starts bent", index + 1);
            if index > 0 && index < EQ_MAX_BANDS - 1 {
                assert_eq!(
                    band.kind,
                    EqBandKind::Bell,
                    "band {} is not a bell",
                    index + 1
                );
            }
        }
        // A ratio rather than a difference: the axis is logarithmic, so
        // "spread out" means each band is a fixed factor above the last.
        for pair in eq.bands.windows(2) {
            assert!(
                pair[1].frequency_hz > pair[0].frequency_hz * 1.5,
                "{} Hz and {} Hz are the same handle as far as a plot is concerned",
                pair[0].frequency_hz,
                pair[1].frequency_hz
            );
        }
        // And the outer two are on the plot rather than off the end of it.
        assert!(eq.bands[0].frequency_hz >= EQ_PLOT_MIN_HZ);
        assert!(eq.bands[EQ_MAX_BANDS - 1].frequency_hz <= EQ_PLOT_MAX_HZ);
    }

    /// The headline of `eq-v2/01`: a lane addresses a band, not a view.
    ///
    /// This replaced `changing_eq_target_rederives_every_selected_band_value`,
    /// which asserted the opposite and was correct about the model it was
    /// written for -- every EQ parameter used to mean "the selected target's",
    /// so moving the selection was how you reached band 2.
    #[test]
    fn a_lane_on_a_band_moves_that_band_whatever_the_face_shows() {
        let mut params = EffectParams::Eq(EqParams::default());
        for target in 0..=EqParams::LOW_PASS_TARGET {
            if let EffectParams::Eq(p) = &mut params {
                p.set_selected_target(target);
            }
            params.set(eq_band_param(3, EQ_BAND_FREQ), 480.0);
            assert_eq!(params.get(eq_band_param(3, EQ_BAND_FREQ)), Some(480.0));
            // And nothing else moved with it. Band 5 rests where the shared
            // table puts it rather than at a number written here: all seven
            // bands start on and spread across the band since 2026-09-15, so
            // a literal would have been this test's own copy of one of them.
            assert_eq!(
                params.get(eq_band_param(4, EQ_BAND_FREQ)),
                Some(EQ_DEFAULT_BAND_HZ[4])
            );
            if let EffectParams::Eq(p) = &mut params {
                p.bands[3].frequency_hz = EQ_DEFAULT_BAND_HZ[3];
            }
        }
    }

    /// No parameter's meaning depends on another parameter's value, asserted
    /// the way the plan asks for it: every id's `get` is unchanged by writing
    /// the selection.
    #[test]
    fn no_eq_parameter_depends_on_the_selection() {
        let mut params = EffectParams::Eq(EqParams::default());
        // Something distinct in every field, so an accidental fallback to a
        // shared default could not pass by coincidence.
        if let EffectParams::Eq(p) = &mut params {
            for (index, band) in p.bands.iter_mut().enumerate() {
                band.enabled = index % 2 == 0;
                band.frequency_hz = 100.0 * (index as f32 + 1.0);
                band.gain_db = index as f32 - 3.0;
                band.q = 0.5 + index as f32;
                band.kind = EqBandKind::from_index(index as i32 % 3);
                band.q_profile = EqQProfile::from_index(index as i32 % 2);
            }
        }
        let ids: Vec<u32> = EffectKind::Eq
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.id)
            .collect();
        let baseline: Vec<Option<f32>> = ids.iter().map(|id| params.get(*id)).collect();
        for target in 0..=EqParams::LOW_PASS_TARGET {
            if let EffectParams::Eq(p) = &mut params {
                p.set_selected_target(target);
            }
            let now: Vec<Option<f32>> = ids.iter().map(|id| params.get(*id)).collect();
            assert_eq!(now, baseline, "selecting target {target} changed a value");
        }
    }

    /// The seven retired ids address a hole, which is what makes shipping
    /// this without a `FORMAT_VERSION` bump the right call: an old lane goes
    /// inert instead of moving some unrelated band. If a future append ever
    /// reaches down into 0..6, this is what says so.
    #[test]
    fn the_retired_eq_ids_address_nothing() {
        let mut params = EffectParams::Eq(EqParams::default());
        let before = params;
        for id in 0..EQ_FIRST {
            assert_eq!(params.get(id), None, "id {id} was retired and answers");
            assert_eq!(params.set(id, 1.0), None, "id {id} was retired and writes");
        }
        assert_eq!(params, before, "a retired id changed the EQ");
    }

    /// A stride of ten with six fields spelled leaves four slots a band, and
    /// they have to be holes rather than silent aliases of a real field.
    #[test]
    fn a_bands_spare_slots_are_holes() {
        let params = EffectParams::Eq(EqParams::default());
        for band in 0..EQ_MAX_BANDS {
            for field in EQ_BAND_FIELDS..EQ_BAND_STRIDE {
                let id = eq_band_param(band, field);
                assert_eq!(eq_band_of(id), None);
                assert_eq!(params.get(id), None, "band {band} slot {field} answers");
            }
        }
    }

    /// The face is a view over a selection and stays one; this is the only
    /// place a selection becomes an id, so it is the only place that can get
    /// it wrong.
    /// The face's indices are the retired ids, which is what let the markup
    /// stay untouched. If that ever stops being true, `eq-device.slint`'s
    /// `modulation-allowed[2]` starts drawing a different knob's arc.
    #[test]
    fn a_face_control_index_round_trips_and_matches_the_retired_ids() {
        for index in 0..EQ_FIRST {
            match EqFaceControl::from_face_index(index) {
                Some(control) => assert_eq!(control.face_index(), index),
                None => assert!(
                    index == EqFaceControl::SELECTOR || index > 6,
                    "index {index} was an id and is now no control at all"
                ),
            }
        }
        assert_eq!(
            EqFaceControl::from_face_index(2),
            Some(EqFaceControl::Frequency),
            "Freq was id 2 and the face still calls it 2"
        );
    }

    #[test]
    fn the_faces_controls_resolve_to_the_selected_target() {
        assert_eq!(
            EqParams::id_for_selected(3, EqFaceControl::Frequency),
            Some(eq_band_param(3, EQ_BAND_FREQ))
        );
        assert_eq!(
            EqParams::id_for_selected(EqParams::LOW_PASS_TARGET, EqFaceControl::Frequency),
            Some(eq_pass_param(EQ_LOW_PASS, EQ_PASS_FREQ))
        );
        // A pass filter has no gain and no Q profile; a band has no slope.
        // `None` rather than a write that lands nowhere, because a control
        // the face greys out should not be reachable by a different route.
        assert_eq!(
            EqParams::id_for_selected(EqParams::HIGH_PASS_TARGET, EqFaceControl::Gain),
            None
        );
        assert_eq!(
            EqParams::id_for_selected(EqParams::HIGH_PASS_TARGET, EqFaceControl::QProfile),
            None
        );
        assert_eq!(EqParams::id_for_selected(0, EqFaceControl::PassSlope), None);
    }

    #[test]
    fn exponential_cutoff_matches_the_uis_perceptual_mapping() {
        // The filter face renders `20 * 1000^x`; the descriptor must agree or
        // the knob and the audio disagree.
        let cutoff = EffectKind::Filter
            .descriptor(FILTER_PARAM_CUTOFF_HZ)
            .unwrap();
        for step in 0..=10 {
            let norm = step as f32 / 10.0;
            let expected = 20.0 * 1000f32.powf(norm);
            let actual = cutoff.from_normalized(norm);
            assert!(
                (actual / expected - 1.0).abs() < 1e-3,
                "at {norm}: descriptor {actual} vs face {expected}"
            );
        }
    }

    #[test]
    fn set_clamps_through_the_descriptor() {
        let mut params = EffectParams::Filter(FilterParams::default());
        assert_eq!(
            params.set(FILTER_PARAM_CUTOFF_HZ, 1_000_000.0),
            Some(20_000.0)
        );
        assert_eq!(params.set(FILTER_PARAM_RESONANCE, -5.0), Some(0.0));
        assert_eq!(params.set(FILTER_PARAM_MODE, 1.0), Some(1.0));
        assert_eq!(params.filter().unwrap().mode, FilterMode::BandPass);
        assert_eq!(params.set(FILTER_PARAM_SLOPE, 1.0), Some(1.0));
        assert_eq!(params.filter().unwrap().slope, FilterSlope::Db24);
        assert_eq!(params.set(FILTER_PARAM_DRIVE, 2.0), Some(1.0));
        assert_eq!(params.filter().unwrap().drive, 1.0);
        assert_eq!(params.set(99, 1.0), None);

        let mut drive = EffectParams::Drive(DriveParams::default());
        drive.set(DRIVE_PARAM_CURVE, 2.0);
        assert_eq!(drive.drive().unwrap().curve, DriveCurve::Fold);
    }

    #[test]
    fn legacy_untagged_filter_params_still_deserialize() {
        // The shape songs used while `Filter` was the only effect kind.
        let legacy = "\
kind = \"filter\"
bypassed = false

[params]
cutoff_hz = 1250.0
resonance = 0.6
mode = \"high_pass\"
";
        let slot: EffectSlotState = toml::from_str(legacy).expect("legacy slot should load");
        assert_eq!(slot.kind(), EffectKind::Filter);
        let filter = slot.params.filter().unwrap();
        assert_eq!(filter.cutoff_hz, 1_250.0);
        assert_eq!(filter.mode, FilterMode::HighPass);
        assert_eq!(filter.slope, FilterSlope::Db12);
        assert_eq!(filter.drive, 0.0);
        assert!(!slot.bypassed);
    }

    /// A manifest written for the generated-room convolution reverb still
    /// loads. Its geometry fields have no counterpart in the feedback delay
    /// network that replaced it and are ignored; the one field that carries
    /// a comparable meaning, `decay_s`, survives, and everything else takes
    /// the new device's default. See `docs/PROJECT_FORMAT.md`.
    #[test]
    fn a_generated_room_manifest_loads_as_the_feedback_network() {
        let legacy = r#"
bypassed = false
wet_dry = 0.25
input_trim = 1.0
output_trim = 1.0

[params]
type = "reverb"

[params.state]
shape = "hall"
material = "brick"
width_m = 18.0
depth_m = 24.0
height_m = 9.0
decay_s = 3.5
capture_x = 0.4
capture_y = 0.8
"#;
        let slot: EffectSlotState = toml::from_str(legacy).unwrap();
        let params = slot.params.reverb().expect("still a reverb");
        assert_eq!(params.decay_s, 3.5, "the one shared field must carry over");
        assert_eq!(params.size, ReverbParams::default().size);
        assert_eq!(params.damping, ReverbParams::default().damping);
        assert_eq!(params.predelay_ms, ReverbParams::default().predelay_ms);
        assert_eq!(params.diffusion, ReverbParams::default().diffusion);
        assert_eq!(params.width, ReverbParams::default().width);
        assert_eq!(params.modulation, ReverbParams::default().modulation);
        assert_eq!(params.low_cut_hz, ReverbParams::default().low_cut_hz);
        assert_eq!(slot.wet_dry, 0.25);
    }

    #[test]
    fn plate_manifest_without_predelay_keeps_the_old_sound() {
        let legacy = r#"
bypassed = false

[params]
type = "plate"

[params.state]
size = 0.5
decay_s = 2.0
damping = 0.4
width = 1.0
"#;
        let slot: EffectSlotState = toml::from_str(legacy).unwrap();
        let plate = slot.params.plate().expect("still a plate");
        assert_eq!(plate.predelay_ms, 0.0);
    }

    /// The retired ids are retired: nothing in the current table may reuse
    /// one, or a route saved against the old room controls would silently
    /// land on a different knob.
    #[test]
    fn reverb_does_not_reuse_its_retired_descriptor_ids() {
        for descriptor in EffectKind::Reverb.descriptors() {
            assert!(
                descriptor.id >= 8,
                "{} reuses retired id {}",
                descriptor.name,
                descriptor.id
            );
        }
        for retired in 0..8u32 {
            assert!(
                EffectKind::Reverb.descriptor(retired).is_none(),
                "id {retired} belonged to the generated-room reverb and must stay unassigned"
            );
        }
    }

    #[test]
    fn tagged_params_round_trip_for_every_kind() {
        for kind in EffectKind::ALL {
            let slot = EffectSlotState::of_kind(kind);
            let text = toml::to_string(&slot).unwrap();
            let back: EffectSlotState = toml::from_str(&text).unwrap();
            assert_eq!(slot, back, "{} did not round-trip:\n{text}", kind.label());
        }
    }
}
