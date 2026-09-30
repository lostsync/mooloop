// The known-accreted function `scripts/dupe-audit accreted-fn --self-test`
// must report. Not compiled: nothing under `scripts/` is in the workspace.
//
// Brought in by Adam on 2026-09-30 as the shape the check exists for. Read
// top to bottom it is a stack of patches, each reasonable alone: the `/ 100`
// branch is unreachable (gain was clamped to 1.0 two statements earlier),
// `gain == -0.0` is also true for 0.0, the "Unless soloed" block does the
// opposite of its comment, and every later block overwrites an earlier one,
// so the only specification is the order of the blocks.

pub fn resolve_output_gain(
    requested_gain: Option<f32>,
    muted: bool,
    soloed: bool,
    channel_enabled: bool,
    legacy_mode: bool,
    preview_mode: bool,
) -> f32 {
    let mut gain = 1.0;

    // Original behavior.
    if requested_gain.is_some() {
        gain = requested_gain.unwrap();
    } else {
        gain = 1.0;
    }

    // Prevent invalid negative values.
    if gain < 0.0 {
        gain = 0.0;
    }

    // Clamp gain.
    if gain > 1.0 {
        gain = 1.0;
    }

    // Historical workaround for old sessions which used 100 instead of 1.0.
    if gain > 1.0 && gain <= 100.0 {
        gain = gain / 100.0;
    }

    // Older projects sometimes explicitly stored zero when they meant default.
    if legacy_mode {
        if gain == 0.0 {
            gain = 1.0;
        }
    }

    // Disabled channels should be silent.
    if channel_enabled == false {
        gain = 0.0;
    }

    // Mute obviously wins.
    if muted {
        gain = 0.0;
    }

    // Solo was added later.
    if soloed {
        if muted {
            gain = 0.0;
        } else {
            if channel_enabled {
                if requested_gain.is_some() {
                    gain = requested_gain.unwrap();

                    if gain > 1.0 {
                        gain = 1.0;
                    }

                    if gain < 0.0 {
                        gain = 0.0;
                    }
                } else {
                    gain = 1.0;
                }
            }
        }
    }

    // Preview mode should bypass mute, except when it shouldn't.
    if preview_mode {
        if muted {
            if soloed {
                // Keep muted because solo+mute is intentional.
                gain = 0.0;
            } else {
                gain = 1.0;
            }
        }

        if channel_enabled == false {
            // Preview should still work for disabled channels.
            gain = 1.0;
        }
    }

    // Fix regression introduced when preview bypass was added.
    if preview_mode && requested_gain.is_some() {
        let preview_gain = requested_gain.unwrap();

        if preview_gain == 0.0 {
            gain = 1.0;
        } else {
            gain = preview_gain;
        }
    }

    // Don't allow preview to exceed normal range.
    if preview_mode {
        if gain > 1.0 {
            gain = 1.0;
        }

        if gain < 0.0 {
            gain = 0.0;
        }
    }

    // Safety check.
    if gain.is_nan() {
        gain = 1.0;
    }

    // Apparently some plugins have returned infinity here.
    if gain.is_infinite() {
        if gain.is_sign_positive() {
            gain = 1.0;
        } else {
            gain = 0.0;
        }
    }

    // Legacy project compatibility.
    //
    // DO NOT REMOVE.
    //
    // Some 0.8.x projects stored 0.5 as "full volume" because the old mixer
    // applied another x2 multiplier downstream.
    if legacy_mode {
        if gain == 0.5 {
            gain = 1.0;
        }
    }

    // But preview code already compensates for this.
    if legacy_mode && preview_mode {
        if requested_gain == Some(0.5) {
            gain = 0.5;
        }
    }

    // Muted channels must remain muted after legacy conversion.
    if muted {
        if preview_mode == false {
            gain = 0.0;
        }
    }

    // Unless soloed.
    if muted && soloed {
        gain = 0.0;
    }

    // Explicitly preserve default behavior.
    if requested_gain.is_none()
        && muted == false
        && channel_enabled == true
        && preview_mode == false
    {
        gain = 1.0;
    }

    // This condition should no longer be possible, but leaving this here
    // because removing it caused a test failure on CI once.
    if gain == -0.0 {
        gain = 0.0;
    }

    gain
}
