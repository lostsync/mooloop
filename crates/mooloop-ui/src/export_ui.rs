//! The export card (MOO-180): its defaults when it opens, Browse, Export and
//! Cancel. The card itself is `ui/export-dialog.slint`; what it sets is read
//! into the session's one value, `RenderSettings`, and the session builds
//! the job from that. Nothing here knows what a job is made of, so a later
//! Rendering issue adds a field there and a control on the card.

use super::*;

/// Wire the export card's callbacks on `window`, as `AppUi::new` does.
pub(crate) fn wire(
    window: &MainWindow,
    state: &Rc<RefCell<UiState>>,
    document_tx: &std::sync::mpsc::Sender<DocumentResult>,
    export_progress: &Rc<RefCell<Option<Arc<ExportProgress>>>>,
    question: &Rc<RefCell<Option<Question>>>,
    export_sample_rate: u32,
) {
    {
        let st = Rc::clone(state);
        let weak = window.as_weak();
        window.on_export_audio(move || {
            if let Some(window) = weak.upgrade() {
                // A second export while one renders reopens the one in
                // flight, with its progress and its Cancel.
                if window.get_export_phase() != 1 {
                    window.set_export_phase(0);
                    show_export_defaults(&window, &st.borrow().session);
                }
                window.set_export_open(true);
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
            let settings = export_settings(&window);
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
/// shows as a placeholder, which the session resolves.
pub(crate) fn export_settings(window: &MainWindow) -> RenderSettings {
    let bitrate = window.get_export_bitrate().clamp(0, MP3_KBPS.len() as i32 - 1) as usize;
    let folder = window.get_export_folder().trim().to_string();
    RenderSettings {
        format: match window.get_export_format() {
            1 => FileFormat::Wav {
                depth: WavDepth::Float32,
            },
            2 => FileFormat::Mp3 {
                kbps: MP3_KBPS[bitrate],
            },
            _ => FileFormat::Wav {
                depth: WavDepth::Pcm24,
            },
        },
        tail: TailSettings {
            max_seconds: window.get_export_tail_seconds().clamp(0, MAX_TAIL_SECONDS as i32) as u32,
        },
        output: OutputSettings {
            folder: (!folder.is_empty()).then(|| PathBuf::from(folder)),
            name: window.get_export_name().to_string(),
        },
        ..RenderSettings::default()
    }
}

/// The export card's defaults for the song as it is now: the folder and
/// name an empty field stands for, and the range the transport plays.
pub(crate) fn show_export_defaults(window: &MainWindow, session: &Session) {
    let defaults = RenderSettings::default();
    let song = session.export_song_name();
    window.set_export_default_folder(session.export_default_folder().display().to_string().into());
    window.set_export_default_name(defaults.stem(song.as_deref()).unwrap_or_default().into());
    window.set_export_range_text(
        match session.export_scope() {
            RenderScope::Song => "The whole song, as the transport plays it".to_string(),
            RenderScope::Pattern { index } => {
                format!("Pattern {}, as the transport plays it", index + 1)
            }
        }
        .into(),
    );
    window.set_export_problem("".into());
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
        card.window.set_export_format(1);

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
        assert!(
            card.window.get_export_range_text().ends_with("as the transport plays it"),
            "{}",
            card.window.get_export_range_text()
        );
        let settings = export_settings(&card.window);
        assert_eq!(settings.output.folder, None);
        assert_eq!(settings.output.name, "");
        assert_eq!(settings.tail.max_seconds, 0);
    }
}
