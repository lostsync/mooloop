//! The export card (MOO-180): its defaults when it opens, Browse, Export and
//! Cancel. The card itself is `ui/export-dialog.slint`; what it sets is read
//! into the session's one value, `RenderSettings`, and the session builds
//! the job from that. Nothing here knows what a job is made of, so a later
//! Rendering issue adds a field there and a control on the card.

use super::*;
use mooloop_session::render_settings::{
    format_bar_beat, parse_bar_beat, Channels, RenderRange, SettingsProblem, Timeline,
};
use std::cell::Cell;

/// The card's Range choices, by their place in the row (MOO-181).
const RANGE_SONG: i32 = 0;
const RANGE_LOOP: i32 = 1;
const RANGE_CUSTOM: i32 = 2;
const RANGE_PATTERN: i32 = 3;

/// Wire the export card's callbacks on `window`, as `AppUi::new` does.
pub(crate) fn wire(
    window: &MainWindow,
    state: &Rc<RefCell<UiState>>,
    document_tx: &std::sync::mpsc::Sender<DocumentResult>,
    export_progress: &Rc<RefCell<Option<Arc<ExportProgress>>>>,
    question: &Rc<RefCell<Option<Question>>>,
    export_sample_rate: u32,
) {
    // Whether a range has been picked on the card. Until one has, the range
    // follows the transport each time the card opens: the whole song in
    // song mode, the current pattern in pattern mode.
    let range_picked = Rc::new(Cell::new(false));
    {
        let st = Rc::clone(state);
        let weak = window.as_weak();
        let range_picked = range_picked.clone();
        window.on_export_audio(move || {
            if let Some(window) = weak.upgrade() {
                // A second export while one renders reopens the one in
                // flight, with its progress and its Cancel.
                if window.get_export_phase() != 1 {
                    window.set_export_phase(0);
                    show_export_defaults(&window, &st.borrow().session, !range_picked.get());
                }
                window.set_export_open(true);
            }
        });
    }
    {
        let st = Rc::clone(state);
        let weak = window.as_weak();
        window.on_export_range_picked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            range_picked.set(true);
            let timeline = st.borrow().session.export_timeline();
            // A custom range starts as the loop selection, which is the
            // stretch most likely to be wanted; with none, it starts as
            // whatever the card was showing.
            if window.get_export_range_index() == RANGE_CUSTOM {
                if let Some((start, end)) = timeline.loop_span() {
                    window.set_export_range_from(format_bar_beat(start).into());
                    window.set_export_range_to(format_bar_beat(end).into());
                }
            }
            show_export_range(&window, &timeline);
        });
    }
    {
        let st = Rc::clone(state);
        let weak = window.as_weak();
        window.on_export_range_edited(move || {
            if let Some(window) = weak.upgrade() {
                show_export_range(&window, &st.borrow().session.export_timeline());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_export_browse_folder(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            // The chooser blocks its thread until it is answered, so it
            // runs on its own, and Browse waits for it (MOO-180).
            window.set_export_browsing(true);
            let weak = window.as_weak();
            std::thread::spawn(move || {
                let picked = pick_folder_dialog("Export to folder");
                let _ = weak.upgrade_in_event_loop(move |window| {
                    window.set_export_browsing(false);
                    match picked {
                        Picked::Path(folder) => {
                            window.set_export_folder(folder.display().to_string().into());
                            window.set_export_problem("".into());
                        }
                        Picked::Cancelled => {}
                        // Not a cancel (MOO-90): the card says why no
                        // chooser came, and the folder can be typed.
                        Picked::Unavailable(none) => {
                            window.set_export_problem(
                                format!("{} Or type the folder.", none.one_line()).into(),
                            );
                        }
                    }
                });
            });
        });
    }
    {
        let progress = export_progress.clone();
        let weak = window.as_weak();
        window.on_export_cancel_render(move || {
            if let Some(progress) = progress.borrow().as_ref() {
                progress.cancel();
            }
            if let Some(window) = weak.upgrade() {
                window.set_status_message("Cancelling the export...".into());
            }
        });
    }
    {
        let st = Rc::clone(state);
        let tx = document_tx.clone();
        let weak = window.as_weak();
        let export_progress = export_progress.clone();
        let question = question.clone();
        window.on_export_confirmed(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let settings = match export_settings(&window) {
                Ok(settings) => settings,
                Err(problem) => {
                    window.set_export_problem(problem.0.into());
                    return;
                }
            };
            // Checked here, with the card still up: a folder that is not
            // there is said on the card rather than after a render.
            let existing = match st.borrow().session.export_request(
                window.get_bpm(),
                window.get_swing_percent(),
                &settings,
            ) {
                Ok(request) => request.job.existing_targets(),
                Err(problem) => {
                    window.set_export_problem(problem.0.into());
                    return;
                }
            };
            window.set_export_problem("".into());
            let st = st.clone();
            let tx = tx.clone();
            let export_progress = export_progress.clone();
            let start = move |window: &MainWindow| {
                // Hosted plugins render in the export too (MOO-81). Their
                // live processors are the engine's, so the export gets
                // processors of second instances, opened here on the
                // control thread with the live ones' state -- which is
                // also captured into the song first, so the request
                // built after it carries it.
                let plugins = st
                    .borrow_mut()
                    .session
                    .export_plugin_processors(export_sample_rate);
                let request = match st.borrow().session.export_request(
                    window.get_bpm(),
                    window.get_swing_percent(),
                    &settings,
                ) {
                    Ok(request) => request,
                    Err(problem) => {
                        window.set_export_problem(problem.0.into());
                        return;
                    }
                };
                if !begin_document_operation(window, "Rendering audio...") {
                    return;
                }
                // The dialog stays up through the render, with its
                // progress and a Cancel, and then shows what the render
                // found (MOO-125). The pump drives it from
                // `export_progress`.
                let progress = Arc::new(ExportProgress::new());
                *export_progress.borrow_mut() = Some(progress.clone());
                window.set_export_progress(-1.0);
                window.set_export_phase(1);
                spawn_document_worker(tx.clone(), "export this song", move || {
                    run_export(request, export_sample_rate, &progress, plugins)
                });
            };
            if existing.is_empty() {
                start(&window);
                return;
            }
            // Replacing asks once, however many files it is (MOO-180).
            let count = existing.len();
            let title = if count == 1 {
                let name = existing[0]
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("Replace \"{name}\"?")
            } else {
                format!("Replace {count} files?")
            };
            ask_question(
                &window,
                &question,
                Question::ReplaceExport(Box::new(start)),
                &title,
                if count == 1 {
                    "A file of that name is already in the folder. Replacing it cannot be undone."
                } else {
                    "Files of those names are already in the folder. Replacing them cannot be undone."
                },
                "Replace",
                "",
            );
        });
    }
}

/// What the export card's settings say, as the session's one value
/// (MOO-180). Folder and name come as typed: empty is the default the card
/// shows as a placeholder, which the session resolves. A custom range that
/// is not bar.beat is the one thing the value cannot hold, so it is the
/// problem instead.
pub(crate) fn export_settings(window: &MainWindow) -> Result<RenderSettings, SettingsProblem> {
    let bitrate = window.get_export_bitrate().clamp(0, MP3_KBPS.len() as i32 - 1) as usize;
    let folder = window.get_export_folder().trim().to_string();
    Ok(RenderSettings {
        range: card_range(window)?,
        format: match window.get_export_format() {
            1 => FileFormat::Mp3 {
                kbps: MP3_KBPS[bitrate],
            },
            _ => FileFormat::Wav {
                depth: match window.get_export_wav_depth() {
                    0 => WavDepth::Pcm16,
                    2 => WavDepth::Float32,
                    _ => WavDepth::Pcm24,
                },
                // The card shows the dither it will use, so it says it.
                dither: Some(window.get_export_dither()),
            },
        },
        channels: if window.get_export_mono() {
            Channels::Mono
        } else {
            Channels::Stereo
        },
        tail: TailSettings {
            max_seconds: window.get_export_tail_seconds().clamp(0, MAX_TAIL_SECONDS as i32) as u32,
        },
        output: OutputSettings {
            folder: (!folder.is_empty()).then(|| PathBuf::from(folder)),
            name: window.get_export_name().to_string(),
        },
        ..RenderSettings::default()
    })
}

/// The range the card's choice names (MOO-181).
fn card_range(window: &MainWindow) -> Result<RenderRange, SettingsProblem> {
    Ok(match window.get_export_range_index() {
        RANGE_SONG => RenderRange::Song,
        RANGE_LOOP => RenderRange::Loop,
        RANGE_CUSTOM => {
            let point = |text: SharedString, which: &str| {
                parse_bar_beat(&text).ok_or_else(|| {
                    SettingsProblem(format!("Type the range's {which} as bar.beat, like 3.1."))
                })
            };
            RenderRange::Custom {
                start_tick: point(window.get_export_range_from(), "start")?,
                end_tick: point(window.get_export_range_to(), "end")?,
            }
        }
        RANGE_PATTERN => RenderRange::Pattern,
        _ => RenderRange::Transport,
    })
}

/// Show the stretch the card's range covers, as bar.beat -- a custom range
/// shows what was typed -- and whether it can be rendered. One that cannot
/// is said on the card, and Export waits for it (MOO-181).
fn show_export_range(window: &MainWindow, timeline: &Timeline) {
    let range = card_range(window);
    if window.get_export_range_index() != RANGE_CUSTOM {
        if let Some((start, end)) = range.as_ref().ok().and_then(|range| range.span(timeline)) {
            window.set_export_range_from(format_bar_beat(start).into());
            window.set_export_range_to(format_bar_beat(end).into());
        }
    }
    let checked = range.and_then(|range| range.scope(timeline));
    window.set_export_range_ok(checked.is_ok());
    window.set_export_problem(checked.err().map(|problem| problem.0).unwrap_or_default().into());
}

/// The export card's defaults for the song as it is now: the folder and
/// name an empty field stands for, and the range. The range follows the
/// transport until one is picked (`follow_transport`); a picked one the
/// song no longer holds -- a loop selection since cleared, a custom range
/// past a shortened song -- is the whole song again.
pub(crate) fn show_export_defaults(window: &MainWindow, session: &Session, follow_transport: bool) {
    let defaults = RenderSettings::default();
    let song = session.export_song_name();
    window.set_export_default_folder(session.export_default_folder().display().to_string().into());
    window.set_export_default_name(defaults.stem(song.as_deref()).unwrap_or_default().into());

    let timeline = session.export_timeline();
    let loop_available = timeline.loop_span().is_some();
    window.set_export_loop_available(loop_available);
    window.set_export_pattern_label(format!("Pattern {}", timeline.pattern + 1).into());
    let index = if follow_transport {
        match timeline.transport {
            RenderScope::Pattern { .. } => RANGE_PATTERN,
            _ => RANGE_SONG,
        }
    } else {
        match (window.get_export_range_index(), card_range(window)) {
            (RANGE_LOOP, _) if !loop_available => RANGE_SONG,
            (RANGE_CUSTOM, Ok(range)) if range.loaded(&timeline) != range => RANGE_SONG,
            (RANGE_CUSTOM, Err(_)) => RANGE_SONG,
            (index, _) => index,
        }
    };
    window.set_export_range_index(index);
    show_export_range(window, &timeline);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_probe::install_backend;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    const RATE: u32 = 48_000;

    struct Card {
        window: MainWindow,
        question: Rc<RefCell<Option<Question>>>,
        results: Receiver<DocumentResult>,
    }

    /// The real window on a fresh song, the export card wired as
    /// `AppUi::new` wires it, and the channel its worker reports on.
    fn card() -> Card {
        install_backend();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let state = Rc::new(RefCell::new(UiState::new(None, RATE, &window)));
        let (tx, results) = channel();
        let progress = Rc::new(RefCell::new(None));
        let question = Rc::new(RefCell::new(None));
        wire(&window, &state, &tx, &progress, &question, RATE);
        window.invoke_export_audio();
        window.set_export_tail_seconds(0);
        Card {
            window,
            question,
            results,
        }
    }

    fn exported(card: &Card) -> Vec<PathBuf> {
        let result = card
            .results
            .recv_timeout(Duration::from_secs(60))
            .expect("the export worker reports");
        // What the pump does with every result.
        card.window.set_document_busy(false);
        match result {
            DocumentResult::Exported { files, cancelled } => {
                assert!(!cancelled);
                files.into_iter().map(|file| file.path).collect()
            }
            DocumentResult::Failed { problem, .. } => {
                panic!("the export failed: {}", problem.message)
            }
            _ => panic!("expected Exported"),
        }
    }

    /// **Export writes the master mix to the folder and name typed on the
    /// card, with no chooser** (MOO-180), and a second export to the same
    /// name asks once before replacing it.
    #[test]
    fn export_writes_the_typed_name_and_asks_before_replacing_it() {
        let card = card();
        let folder = tempfile::tempdir().unwrap();
        assert!(card.window.get_export_open());
        card.window.set_export_folder(folder.path().display().to_string().into());
        card.window.set_export_name("take one".into());
        card.window.set_export_wav_depth(2);

        card.window.invoke_export_confirmed();
        assert_eq!(card.window.get_export_phase(), 1, "the card shows the render");
        let target = folder.path().join("take one.wav");
        assert_eq!(exported(&card), std::slice::from_ref(&target));
        assert!(target.is_file());

        // Again, to the same name: asked, not rendered.
        card.window.set_export_phase(0);
        card.window.invoke_export_confirmed();
        assert!(card.window.get_question_open());
        assert_eq!(card.window.get_question_title(), "Replace \"take one.wav\"?");
        assert!(card.results.try_recv().is_err(), "nothing renders before the answer");
        let Some(Question::ReplaceExport(start)) = card.question.borrow_mut().take() else {
            panic!("the question is the export's");
        };
        start(&card.window);
        assert_eq!(exported(&card), [target]);
    }

    /// A folder that isn't there is said on the card, and nothing renders.
    #[test]
    fn a_missing_folder_is_said_on_the_card() {
        let card = card();
        let folder = tempfile::tempdir().unwrap();
        card.window
            .set_export_folder(folder.path().join("gone").display().to_string().into());
        card.window.invoke_export_confirmed();
        assert!(card.window.get_export_problem().contains("doesn't exist"));
        assert_eq!(card.window.get_export_phase(), 0);
        assert!(card.results.try_recv().is_err());
    }

    /// Empty fields are the song's defaults, shown as placeholders, and the
    /// range says what the transport plays.
    #[test]
    fn empty_fields_show_the_defaults() {
        let card = card();
        assert_eq!(
            card.window.get_export_default_folder(),
            Session::default().export_default_folder().display().to_string()
        );
        assert_eq!(card.window.get_export_default_name(), "mooloop-export");
        let settings = export_settings(&card.window).unwrap();
        assert_eq!(settings.output.folder, None);
        assert_eq!(settings.output.name, "");
        assert_eq!(settings.tail.max_seconds, 0);
    }

    /// **The card's format row reads into the settings** (MOO-186): WAV
    /// depth and dither, MP3 bitrate, and mono for either.
    #[test]
    fn the_format_row_reads_depth_dither_and_mono() {
        let card = card();
        let window = &card.window;
        let settings = export_settings(window).unwrap();
        assert_eq!(
            settings.format,
            FileFormat::Wav {
                depth: WavDepth::Pcm24,
                dither: Some(false),
            },
            "the card opens on today's export, 24-bit undithered"
        );
        assert_eq!(settings.channels, Channels::Stereo);

        window.set_export_wav_depth(0);
        window.set_export_dither(true);
        window.set_export_mono(true);
        let settings = export_settings(window).unwrap();
        assert_eq!(
            settings.format,
            FileFormat::Wav {
                depth: WavDepth::Pcm16,
                dither: Some(true),
            }
        );
        assert_eq!(settings.channels, Channels::Mono);

        window.set_export_format(1);
        window.set_export_bitrate(0);
        let settings = export_settings(window).unwrap();
        assert_eq!(settings.format, FileFormat::Mp3 { kbps: 192 });
        assert_eq!(settings.channels, Channels::Mono);
    }

    /// The window and its session, for the range tests, which set up the
    /// song before the card opens.
    fn card_on(session: impl FnOnce(&mut Session)) -> (Card, Rc<RefCell<UiState>>) {
        install_backend();
        let window = MainWindow::new().expect("the testing backend builds a window");
        let state = Rc::new(RefCell::new(UiState::new(None, RATE, &window)));
        session(&mut state.borrow_mut().session);
        let (tx, results) = channel();
        let progress = Rc::new(RefCell::new(None));
        let question = Rc::new(RefCell::new(None));
        wire(&window, &state, &tx, &progress, &question, RATE);
        window.invoke_export_audio();
        window.set_export_tail_seconds(0);
        (
            Card {
                window,
                question,
                results,
            },
            state,
        )
    }

    fn four_bars_looping_two(session: &mut Session) {
        use mooloop_core::{LoopRange, PatternPlacement, TICKS_PER_BAR};
        session.song_mode = true;
        session.current_pattern = 0;
        session.pattern_lengths[0] = 16;
        session.playlist = (0..4)
            .map(|bar| PatternPlacement::new(0, bar * TICKS_PER_BAR))
            .collect();
        session.loop_range = LoopRange {
            start_tick: TICKS_PER_BAR,
            end_tick: 3 * TICKS_PER_BAR,
            enabled: false,
        };
    }

    /// **The range follows the transport until one is picked, and a custom
    /// one starts as the loop selection** (MOO-181).
    #[test]
    fn the_range_follows_the_transport_and_custom_starts_as_the_loop() {
        let (card, state) = card_on(four_bars_looping_two);
        let window = &card.window;
        assert_eq!(window.get_export_range_index(), RANGE_SONG);
        assert!(window.get_export_loop_available());
        assert_eq!(window.get_export_range_from(), "1.1");
        assert_eq!(window.get_export_range_to(), "5.1");

        // Pattern mode, and the card opened again, before anything is
        // picked: the current pattern.
        state.borrow_mut().session.song_mode = false;
        window.invoke_export_audio();
        assert_eq!(window.get_export_range_index(), RANGE_PATTERN);
        assert_eq!(window.get_export_pattern_label(), "Pattern 1");

        window.set_export_range_index(RANGE_CUSTOM);
        window.invoke_export_range_picked();
        assert_eq!(window.get_export_range_from(), "2.1");
        assert_eq!(window.get_export_range_to(), "4.1");
        assert!(window.get_export_range_ok());
        let settings = export_settings(window).unwrap();
        assert_eq!(
            settings.range,
            RenderRange::Custom {
                start_tick: mooloop_core::TICKS_PER_BAR,
                end_tick: 3 * mooloop_core::TICKS_PER_BAR,
            }
        );
        // Picked, so it stays picked when the card opens again.
        window.invoke_export_audio();
        assert_eq!(window.get_export_range_index(), RANGE_CUSTOM);
    }

    /// **A custom range that ends at or before its start cannot be
    /// confirmed** (MOO-181): the card says why, Export waits, and nothing
    /// renders even if confirm is reached another way.
    #[test]
    fn a_backwards_custom_range_cannot_be_confirmed() {
        let (card, _state) = card_on(four_bars_looping_two);
        let folder = tempfile::tempdir().unwrap();
        let window = &card.window;
        window.set_export_folder(folder.path().display().to_string().into());
        window.set_export_range_index(RANGE_CUSTOM);
        window.invoke_export_range_picked();
        for (from, to, problem) in [
            ("3.1", "3.1", "The range must end after it starts."),
            ("3.1", "2.3", "The range must end after it starts."),
            ("3.1", "6.1", "The song ends at 5.1."),
            ("3.1", "later", "Type the range's end as bar.beat, like 3.1."),
        ] {
            window.set_export_range_from(from.into());
            window.set_export_range_to(to.into());
            window.invoke_export_range_edited();
            assert!(!window.get_export_range_ok(), "{from} to {to}");
            assert_eq!(window.get_export_problem(), problem);
            window.invoke_export_confirmed();
            assert_eq!(window.get_export_phase(), 0, "{from} to {to} started a render");
            assert!(card.results.try_recv().is_err());
        }
        window.set_export_range_to("4.1".into());
        window.invoke_export_range_edited();
        // The export takes its tempo from the window, as the app does.
        window.set_bpm(120);
        assert!(window.get_export_range_ok());
        assert_eq!(window.get_export_problem(), "");
        window.invoke_export_confirmed();
        let written = exported(&card);
        let reader = hound::WavReader::open(&written[0]).unwrap();
        // One bar at 120 BPM and 48 kHz.
        assert_eq!(reader.duration(), 96_000);
    }

    /// The loop choice is unavailable while the song has no loop selection,
    /// and a picked one that has gone opens as the whole song.
    #[test]
    fn the_loop_choice_needs_a_loop_selection() {
        let (card, state) = card_on(four_bars_looping_two);
        let window = &card.window;
        window.set_export_range_index(RANGE_LOOP);
        window.invoke_export_range_picked();
        assert_eq!(window.get_export_range_from(), "2.1");
        assert_eq!(window.get_export_range_to(), "4.1");

        state.borrow_mut().session.loop_range = mooloop_core::LoopRange::default();
        window.invoke_export_audio();
        assert!(!window.get_export_loop_available());
        assert_eq!(window.get_export_range_index(), RANGE_SONG);
        assert!(window.get_export_range_ok());
    }
}
