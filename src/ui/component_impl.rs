use super::{
    State,
    lucide::{FULLSCREEN, INFO, TEXT_ALIGN_JUSTIFY, X},
    open_media,
};
use crate::ui::theme::{GLASS_SURFACE, SURFACE_BORDER, SURFACE_OPAQUE, glass_capsule};
use crate::utils::format_time;
use rune::{El, col, color::hex, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};

pub(crate) fn top_bar(state: &State, kit: &Kit) -> El {
    let title = state
        .playlist
        .current_index()
        .and_then(|index| state.playlist.tracks().get(index))
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "Veyra".into());
    let fullscreen = glass_capsule(
        IconButton::new(&FULLSCREEN)
            .label("Fullscreen")
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Large)
            .on_press_tx(|_: &mut State, tx| tx.toggle_full_screen())
            .build(kit)
            .radius(999.0),
    );
    let info = glass_capsule(
        IconButton::new(&INFO)
            .label(if state.statistics_open {
                "Hide statistics"
            } else {
                "Show statistics"
            })
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Large)
            .on_press(|s: &mut State| s.statistics_open = !s.statistics_open)
            .build(kit)
            .radius(999.0),
    );
    row()
        .pin(Some(18.0), Some(18.0), Some(18.0), None)
        .gap(10.0)
        .center()
        .child(fullscreen)
        .child(
            row().grow(1.0).min_w(0.0).center().child(
                row()
                    .max_w(620.0)
                    .min_w(120.0)
                    .shrink()
                    .px(20.0)
                    .py(12.0)
                    .radius(999.0)
                    .backdrop_blur(12.0)
                    .fill(hex(GLASS_SURFACE).unwrap())
                    .border(1.0, hex(SURFACE_BORDER).unwrap())
                    .child(
                        text(title)
                            .font(kit.ty.ui)
                            .color(kit.c.text_base)
                            .truncate(),
                    ),
            ),
        )
        .child(info)
}

pub(crate) fn options_button(state: &State, kit: &Kit) -> El {
    row()
        .pin(Some(18.0), None, None, Some(34.0))
        .child(glass_capsule(
            IconButton::new(&TEXT_ALIGN_JUSTIFY)
                .label(if state.options_open {
                    "Hide options"
                } else {
                    "Show options"
                })
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| s.options_open = !s.options_open)
                .build(kit)
                .radius(999.0),
        ))
}

pub(crate) fn options_view(state: &State, kit: &Kit) -> El {
    col()
        .key("options-overlay")
        .w(330.0)
        .pin(Some(18.0), None, None, Some(92.0))
        .px(8.0)
        .py(8.0)
        .gap(2.0)
        .radius(14.0)
        .backdrop_blur(14.0)
        .fill(hex(SURFACE_OPAQUE).unwrap())
        .border(1.0, hex(SURFACE_BORDER).unwrap())
        .child(menu_item(kit, "Open media", "Ctrl+O", |s| {
            s.options_open = false;
            open_media(s);
        }))
        .child(menu_item(kit, "Reload tools", "R", |s| {
            s.scripts.reload();
            s.status = "Tools reloaded".into();
        }))
        .child(separator())
        .child(menu_item(
            kit,
            if state.controls_visible {
                "Hide controls"
            } else {
                "Show controls"
            },
            "H",
            |s| s.controls_visible = !s.controls_visible,
        ))
        .child(menu_item(kit, "Fullscreen", "F", |s| {
            s.options_open = false
        }))
        .child(separator())
        .child(menu_item(kit, "Keyboard shortcuts", "Ctrl+?", |s| {
            s.status = "Space Play/Pause · ←/→ Seek · F Fullscreen · M Mute · R Reload tools".into()
        }))
}

fn separator() -> El {
    el().w_full().h(1.0).my(3.0).fill(hex("#ffffff1f").unwrap())
}

fn menu_item(kit: &Kit, label: &str, shortcut: &str, action: impl Fn(&mut State) + 'static) -> El {
    row()
        .w_full()
        .h(36.0)
        .px(10.0)
        .gap(12.0)
        .center()
        .radius(8.0)
        .hover(hex("#ffffff1f").unwrap())
        .on_press(action)
        .child(text(label).font(kit.ty.body).color(kit.c.text_base))
        .child(
            row()
                .grow(1.0)
                .justify_end()
                .child(text(shortcut).font(kit.ty.small).color(kit.c.text_faint)),
        )
}

pub(crate) fn statistics_view(state: &State, kit: &Kit) -> El {
    let tabs = col()
        .w(112.0)
        .gap(4.0)
        .child(tab(state, kit, 0, "Overview"))
        .child(tab(state, kit, 1, "Video"))
        .child(tab(state, kit, 2, "Audio"))
        .child(tab(state, kit, 3, "Performance"))
        .child(tab(state, kit, 4, "Buffering"))
        .child(tab(state, kit, 5, "Streams"));
    let content = col()
        .grow(1.0)
        .min_w(0.0)
        .pl(14.0)
        .clip_rounded(0.0)
        .child(page(state, kit));
    col()
        .key("statistics-overlay")
        .w(440.0)
        .pin(None, Some(82.0), Some(18.0), None)
        .px(18.0)
        .py(20.0)
        .gap(8.0)
        .radius(20.0)
        .fill(hex(SURFACE_OPAQUE).unwrap())
        .border(1.0, hex(SURFACE_BORDER).unwrap())
        .child(
            row()
                .w_full()
                .justify_between()
                .child(text("Statistics").font(kit.ty.ui).color(kit.c.text_base))
                .child(
                    IconButton::new(&X)
                        .label("Close statistics")
                        .variant(IconButtonVariant::GhostMuted)
                        .on_press(|s: &mut State| s.statistics_open = false)
                        .build(kit)
                        .radius(999.0),
                ),
        )
        .child(
            row()
                .w_full()
                .gap(12.0)
                .child(tabs)
                .child(el().w(1.0).h_full().fill(hex("#ffffff14").unwrap()))
                .child(content),
        )
        .child(
            text("Unavailable metrics are shown as —")
                .w_full()
                .wrap_anywhere()
                .font(kit.ty.small)
                .color(kit.c.text_faint),
        )
}

fn tab(state: &State, kit: &Kit, page: usize, label: &str) -> El {
    Button::new(label)
        .variant(if state.statistics_page == page {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut State| s.statistics_page = page)
        .build(kit)
        .w_full()
}
fn row_value(kit: &Kit, name: &str, value: impl Into<String>) -> El {
    row()
        .w_full()
        .gap(8.0)
        .child(
            text(name)
                .shrink()
                .font(kit.ty.small)
                .color(kit.c.text_muted),
        )
        .child(
            row().grow(1.0).min_w(0.0).justify_end().child(
                text(value.into())
                    .max_w(230.0)
                    .shrink()
                    .wrap_anywhere()
                    .font(kit.ty.small)
                    .color(kit.c.text_base),
            ),
        )
}
fn page(state: &State, kit: &Kit) -> El {
    let s = &state.statistics;
    let optional = |v: Option<String>| v.unwrap_or_else(|| "—".into());
    match state.statistics_page {
        0 => col()
            .gap(5.0)
            .child(text("Overview").font(kit.ty.small).color(kit.c.bg_accent))
            .child(row_value(kit, "File", optional(s.file_name.clone())))
            .child(row_value(kit, "Path", optional(s.file_path.clone())))
            .child(row_value(kit, "Container", optional(s.container.clone())))
            .child(row_value(kit, "Position", format_time(s.position)))
            .child(row_value(
                kit,
                "Remaining",
                s.duration
                    .map(|d| format_time((d - s.position).max(0.0)))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(kit, "State", s.playback_state.clone()))
            .child(row_value(
                kit,
                "Resolution",
                match (s.video_width, s.video_height) {
                    (Some(w), Some(h)) => format!("{w} × {h}"),
                    _ => "—".into(),
                },
            ))
            .child(row_value(
                kit,
                "Video codec",
                optional(s.video_codec.clone()),
            ))
            .child(row_value(
                kit,
                "Audio codec",
                optional(s.audio_codec.clone()),
            )),
        1 => col()
            .gap(5.0)
            .child(
                text("Video diagnostics")
                    .font(kit.ty.small)
                    .color(kit.c.bg_accent),
            )
            .child(row_value(kit, "Codec", optional(s.video_codec.clone())))
            .child(row_value(kit, "Profile", optional(s.video_profile.clone())))
            .child(row_value(
                kit,
                "Pixel format",
                optional(s.video_pixel_format.clone()),
            ))
            .child(row_value(
                kit,
                "FPS",
                s.video_fps
                    .map(|v| format!("{v:.2}"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Bitrate",
                s.video_bitrate
                    .map(|v| format!("{v} bps"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(kit, "Decoder", optional(s.video_decoder.clone())))
            .child(row_value(
                kit,
                "Frames processed",
                s.video_frames_processed.to_string(),
            ))
            .child(row_value(
                kit,
                "Processing",
                s.frame_processing_ms
                    .map(|v| format!("{v:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
            )),
        2 => col()
            .gap(5.0)
            .child(
                text("Audio diagnostics")
                    .font(kit.ty.small)
                    .color(kit.c.bg_accent),
            )
            .child(row_value(kit, "Codec", optional(s.audio_codec.clone())))
            .child(row_value(
                kit,
                "Bitrate",
                s.audio_bitrate
                    .map(|v| format!("{v} bps"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Sample rate",
                s.audio_sample_rate
                    .map(|v| format!("{v} Hz"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Channels",
                s.audio_channels
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(kit, "Output", optional(s.audio_output.clone())))
            .child(row_value(
                kit,
                "Processing",
                s.audio_processing_ms
                    .map(|v| format!("{v:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
            )),
        3 => col()
            .gap(5.0)
            .child(
                text("Performance")
                    .font(kit.ty.small)
                    .color(kit.c.bg_accent),
            )
            .child(row_value(
                kit,
                "Current frame",
                s.frame_processing_ms
                    .map(|v| format!("{v:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Average frame",
                s.frame_processing_avg_ms
                    .map(|v| format!("{v:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Peak frame",
                s.frame_processing_peak_ms
                    .map(|v| format!("{v:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(
                kit,
                "Frames processed",
                s.video_frames_processed.to_string(),
            ))
            .child(row_value(
                kit,
                "Dropped frames",
                s.dropped_frames
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(row_value(kit, "Sampling", "250 ms")),
        4 => col()
            .gap(5.0)
            .child(
                text("Buffering and data flow")
                    .font(kit.ty.small)
                    .color(kit.c.bg_accent),
            )
            .child(row_value(kit, "Buffering state", "—"))
            .child(row_value(kit, "Packet queues", "—"))
            .child(row_value(kit, "Buffered duration", "—"))
            .child(
                text("Veyra currently decodes directly without a buffering or queue subsystem.")
                    .w_full()
                    .wrap_anywhere()
                    .font(kit.ty.small)
                    .color(kit.c.text_muted),
            ),
        _ => col()
            .gap(5.0)
            .child(
                text("Selected streams")
                    .font(kit.ty.small)
                    .color(kit.c.bg_accent),
            )
            .child(row_value(
                kit,
                "Video stream",
                optional(s.video_codec.clone()),
            ))
            .child(row_value(
                kit,
                "Audio stream",
                optional(s.audio_codec.clone()),
            ))
            .child(row_value(
                kit,
                "Audio sample rate",
                s.audio_sample_rate
                    .map(|v| format!("{v} Hz"))
                    .unwrap_or_else(|| "—".into()),
            ))
            .child(
                text("Only selected best audio/video streams are currently exposed.")
                    .w_full()
                    .wrap_anywhere()
                    .font(kit.ty.small)
                    .color(kit.c.text_muted),
            ),
    }
}
