use self::lucide::{
    FAST_FORWARD, FOLDER_OPEN, FULLSCREEN, LIST, PAUSE, PLAY, REWIND, SKIP_BACK, SKIP_FORWARD,
    VOLUME_2, VOLUME_X, X,
};
use self::styles::glass_capsule;
use crate::utils::format_time;
use crate::{
    playback::Engine,
    playlist::{Playlist, is_probably_media},
};
use rune::color::hex;
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::{
    AppSpec, Cx, El, NativeSurface, ScrollSpec, SurfaceFit, Tx, Waker, col, el, icon, row, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, PaletteSeed,
};
use std::{path::PathBuf, process::Command, time::Duration};

mod lucide;
mod styles;

pub struct State {
    engine: Engine,
    playlist: Playlist,
    surface: NativeSurface,
    status: String,
    volume: f64,
    frame_id: u64,
    last_paused: bool,
    last_muted: bool,
    last_error: Option<String>,
    last_position: f64,
    last_duration: Option<f64>,
    playlist_open: bool,
    controls_visible: bool,
    volume_hovered: bool,
}

pub fn run() {
    rune::set_app_id("Veyra");
    rune::run(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Veyra", (820.0, 480.0), State::new, view)
    });
}

impl State {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            playlist: Playlist::default(),
            surface: NativeSurface::new(),
            status: "Open a media file to begin".into(),
            volume: 100.0,
            frame_id: 0,
            last_paused: false,
            last_muted: false,
            last_error: None,
            last_position: 0.0,
            last_duration: None,
            playlist_open: false,
            controls_visible: true,
            volume_hovered: false,
        }
    }
}

fn setup(state: &mut State, waker: Waker) {
    state.surface.set_waker(waker);
}

fn poll(state: &mut State, _: f64, tx: &mut Tx) -> bool {
    tx.wake_after(Duration::from_millis(16));
    let mut changed = false;
    if let Some(frame) = state.engine.take_latest_frame() {
        state.frame_id = state.frame_id.wrapping_add(1);
        let result = state.surface.push_bytes(
            ByteFrame {
                width: frame.width,
                height: frame.height,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [frame.rgba, Vec::new()],
            },
            state.frame_id,
        );
        if let Err(error) = result {
            state.status = error;
        }
        changed = true;
    }
    let snapshot = state.engine.snapshot();
    if state.last_paused != snapshot.paused
        || state.last_muted != snapshot.muted
        || state.last_error != snapshot.error
        || (state.last_position - snapshot.position).abs() > 0.02
        || state.last_duration != snapshot.duration
    {
        state.last_paused = snapshot.paused;
        state.last_muted = snapshot.muted;
        state.last_error = snapshot.error.clone();
        state.last_position = snapshot.position;
        state.last_duration = snapshot.duration;
        if let Some(error) = snapshot.error {
            state.status = error;
        }
        changed = true;
    }
    changed
}

fn on_key(state: &mut State, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
    match &event.key {
        rune::Key::Text(value) if value == " " => {
            state.engine.toggle_pause();
            true
        }
        rune::Key::Left => {
            state.engine.seek(-10.0);
            true
        }
        rune::Key::Right => {
            state.engine.seek(10.0);
            true
        }
        rune::Key::Text(value) if value.eq_ignore_ascii_case("f") => {
            tx.toggle_full_screen();
            true
        }
        rune::Key::Text(value) if value.eq_ignore_ascii_case("m") => {
            state.engine.toggle_mute();
            true
        }
        _ => false,
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let kit = Kit::from_palette(PaletteSeed {
        bg: hex("#101217").unwrap(),
        fg: hex("#f2f4f8").unwrap(),
        accent: hex("#8fb7ff").unwrap(),
    });
    let root = el()
        .w_full()
        .h_full()
        .fill(kit.c.bg_deep)
        .child(video_view(state, &kit))
        .child(title_overlay(state, &kit))
        .child(top_left_controls(&kit))
        .child(top_right_controls(state, &kit))
        .child(if state.controls_visible {
            controls(state, &kit)
        } else {
            el()
        })
        .child(timeline(state, &kit))
        .child(volume_control(state, &kit))
        .child(if state.playlist_open {
            playlist_view(state, &kit)
        } else {
            el()
        });
    let _ = cx;
    rune_kit::root(root, Vec::new())
}

fn video_view(state: &State, kit: &Kit) -> El {
    let surface = if state.frame_id > 0 {
        el().key("video-frame")
            .w_full()
            .h_full()
            .native_surface(&state.surface, SurfaceFit::Fit)
    } else {
        col()
            .w_full()
            .h_full()
            .center()
            .gap(12.0)
            .child(icon(&FOLDER_OPEN, 38.0).color(kit.c.icon_muted))
            .child(
                text("No media loaded")
                    .font(kit.ty.ui)
                    .color(kit.c.text_base),
            )
            .child(
                text(state.status.clone())
                    .font(kit.ty.body)
                    .color(kit.c.text_muted),
            )
            .child(
                Button::new("Open media")
                    .variant(ButtonVariant::Contrast)
                    .on_press(|s: &mut State| open_media(s))
                    .control_size(ControlSize::Large)
                    .build(kit),
            )
    };
    el().key("video-viewport")
        .w_full()
        .h_full()
        .center()
        .child(surface)
}

fn title_overlay(state: &State, kit: &Kit) -> El {
    let title = state
        .playlist
        .current_index()
        .and_then(|index| state.playlist.tracks().get(index))
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "Veyra".into());
    row()
        .pin(Some(0.0), Some(18.0), Some(0.0), None)
        .center()
        .child(
            row()
                .max_w(620.0)
                .px(20.0)
                .py(12.0)
                .radius(18.0)
                .backdrop_blur(10.0)
                .fill(hex("#090b1080").unwrap())
                .child(
                    text(title)
                        .font(kit.ty.ui)
                        .color(kit.c.text_base)
                        .truncate(),
                ),
        )
}

fn top_left_controls(kit: &Kit) -> El {
    row()
        .pin(Some(22.0), Some(22.0), None, None)
        .child(glass_capsule(
            IconButton::new(&FULLSCREEN)
                .label("Fullscreen")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press_tx(|_: &mut State, tx| tx.toggle_full_screen())
                .build(kit)
                .radius(999.0),
        ))
}

fn top_right_controls(state: &State, kit: &Kit) -> El {
    row()
        .pin(None, Some(22.0), Some(22.0), None)
        .child(glass_capsule(
            IconButton::new(&LIST)
                .label(if state.playlist_open {
                    "Hide playlist"
                } else {
                    "Show playlist"
                })
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| s.playlist_open = !s.playlist_open)
                .build(kit)
                .radius(999.0),
        ))
}

fn volume_control(state: &State, kit: &Kit) -> El {
    let slider = if state.volume_hovered {
        let level = (state.volume / 100.0).clamp(0.0, 1.0) as f32;
        col()
            .key("volume-slider")
            .w(36.0)
            .h(132.0)
            .center()
            .radius(999.0)
            .backdrop_blur(12.0)
            .fill(hex("#090b1080").unwrap())
            .child(
                el().key("volume-track")
                    .w(6.0)
                    .h(104.0)
                    .radius(999.0)
                    .fill(hex("#ffffff26").unwrap())
                    .on_drag(move |s: &mut State, drag, _| {
                        if matches!(drag.phase, rune::Phase::Down | rune::Phase::Move) {
                            let normalized = 1.0 - (drag.y as f64 / 104.0);
                            adjust_volume_to(s, normalized.clamp(0.0, 1.0) * 100.0);
                        }
                    })
                    .child(
                        el().w(6.0)
                            .h((104.0 * level).max(1.0))
                            .pin(Some(0.0), None, Some(0.0), Some(0.0))
                            .radius(999.0)
                            .fill(kit.c.bg_accent),
                    )
                    .child(
                        el().w(16.0)
                            .h(16.0)
                            .absolute(-5.0, (104.0 - 16.0) * (1.0 - level))
                            .radius(999.0)
                            .fill(kit.c.bg_accent),
                    ),
            )
    } else {
        el()
    };
    col()
        .pin(None, None, Some(24.0), Some(30.0))
        .gap(8.0)
        .center()
        .on_hover_change(|s: &mut State, inside, _| s.volume_hovered = inside)
        .child(slider)
        .child(glass_capsule(
            IconButton::new(if state.engine.snapshot().muted {
                &VOLUME_X
            } else {
                &VOLUME_2
            })
            .label(if state.engine.snapshot().muted {
                "Unmute"
            } else {
                "Mute"
            })
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Large)
            .on_press(|s: &mut State| s.engine.toggle_mute())
            .build(kit)
            .radius(999.0),
        ))
}

fn playlist_view(state: &State, kit: &Kit) -> El {
    let rows = state
        .playlist
        .tracks()
        .iter()
        .enumerate()
        .map(|(index, track)| {
            let active = state.playlist.current_index() == Some(index);
            let variant = if active {
                ButtonVariant::Contrast
            } else {
                ButtonVariant::GhostMuted
            };
            Button::new(track.title.clone())
                .variant(variant)
                .help(track.path.display().to_string())
                .control_size(ControlSize::Large)
                .on_press(move |s: &mut State| open_track(s, index))
                .build(kit)
        });
    let list = if state.playlist.is_empty() {
        col().grow(1.0).min_h(0.0).center().child(
            text("Your playlist is empty")
                .font(kit.ty.small)
                .color(kit.c.text_muted),
        )
    } else {
        col()
            .key("playlist-scroll")
            .grow(1.0)
            .min_h(0.0)
            .gap(6.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .children(rows)
    };
    col()
        .key("playlist-overlay")
        .w(360.0)
        .h(500.0)
        .pin(None, Some(22.0), Some(22.0), None)
        .px(18.0)
        .py(20.0)
        .gap(16.0)
        .radius(20.0)
        .backdrop_blur(14.0)
        .fill(hex("#11141b9c").unwrap())
        .child(
            row()
                .w_full()
                .justify_between()
                .child(text("Playlist").font(kit.ty.ui).color(kit.c.text_base))
                .child(
                    row()
                        .gap(8.0)
                        .center()
                        .child(
                            text(state.playlist.tracks().len().to_string())
                                .font(kit.ty.small)
                                .color(kit.c.text_faint),
                        )
                        .child(
                            IconButton::new(&X)
                                .label("Close playlist")
                                .variant(IconButtonVariant::GhostMuted)
                                .on_press(|s: &mut State| s.playlist_open = false)
                                .build(kit)
                                .radius(999.0),
                        ),
                ),
        )
        .child(list)
}

fn controls(state: &State, kit: &Kit) -> El {
    let snapshot = state.engine.snapshot();
    let playing = state.frame_id > 0 && !snapshot.paused;
    let play_icon = if playing { &PAUSE } else { &PLAY };
    let transport = row()
        .gap(10.0)
        .center()
        .child(
            IconButton::new(&SKIP_BACK)
                .label("Previous track")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| select_relative(s, -1))
                .build(kit)
                .radius(999.0),
        )
        .child(
            IconButton::new(&REWIND)
                .label("Seek backward ten seconds")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| s.engine.seek(-10.0))
                .build(kit)
                .radius(999.0),
        )
        .child(
            IconButton::new(play_icon)
                .label(if playing { "Pause" } else { "Play" })
                .control_size(ControlSize::Large)
                .variant(IconButtonVariant::Contrast)
                .on_press(|s: &mut State| s.engine.toggle_pause())
                .build(kit)
                .radius(999.0),
        )
        .child(
            IconButton::new(&FAST_FORWARD)
                .label("Seek forward ten seconds")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| s.engine.seek(10.0))
                .build(kit)
                .radius(999.0),
        )
        .child(
            IconButton::new(&SKIP_FORWARD)
                .label("Next track")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut State| select_relative(s, 1))
                .build(kit)
                .radius(999.0),
        );
    row()
        .pin(Some(0.0), None, Some(0.0), Some(34.0))
        .w_full()
        .center()
        .child(
            row()
                .px(16.0)
                .py(12.0)
                .gap(10.0)
                .center()
                .radius(999.0)
                .backdrop_blur(18.0)
                .fill(hex("#090b1060").unwrap())
                .child(transport),
        )
}

fn timeline(state: &State, kit: &Kit) -> El {
    let snapshot = state.engine.snapshot();
    let progress = snapshot
        .duration
        .filter(|duration| *duration > 0.0)
        .map_or(0.0, |duration| {
            (snapshot.position / duration).clamp(0.0, 1.0) as f32
        });
    let duration_for_drag = snapshot.duration.filter(|duration| *duration > 0.0);
    let timeline_width = 680.0;
    let seek = el()
        .w(timeline_width)
        .h(16.0)
        .on_drag(move |s: &mut State, drag, _| {
            if let Some(duration) = duration_for_drag {
                if matches!(drag.phase, rune::Phase::Down | rune::Phase::Move) {
                    let target =
                        (drag.x as f64 / drag.w.max(1.0) as f64 * duration).clamp(0.0, duration);
                    s.engine.seek_to(target);
                }
            }
        })
        .child(
            el().absolute(0.0, 5.0)
                .w(timeline_width)
                .h(6.0)
                .radius(3.0)
                .fill(hex("#ffffff26").unwrap()),
        )
        .child(
            el().absolute(0.0, 5.0)
                .w((timeline_width * progress).max(1.0))
                .h(6.0)
                .radius(3.0)
                .fill(kit.c.bg_accent),
        )
        .child(
            el().absolute(timeline_width * progress - 6.0, 2.0)
                .w(12.0)
                .h(12.0)
                .radius(999.0)
                .fill(hex("#ffffff").unwrap()),
        );
    col()
        .pin(Some(0.0), None, Some(0.0), Some(92.0))
        .w_full()
        .center()
        .gap(8.0)
        .child(seek)
        .child(
            row()
                .w(680.0)
                .justify_between()
                .child(
                    text(format_time(snapshot.position))
                        .font(kit.ty.body)
                        .color(kit.c.text_muted),
                )
                .child(
                    text(format_time(snapshot.duration.unwrap_or(0.0)))
                        .font(kit.ty.body)
                        .color(kit.c.text_muted),
                ),
        )
}

fn open_media(state: &mut State) {
    if let Some(path) = pick_media_file() {
        open_path(state, path);
    }
}

fn open_path(state: &mut State, path: PathBuf) {
    if !is_probably_media(&path) {
        state.status = "Unsupported file type".into();
        return;
    }
    state.playlist.add(path.clone());
    state.status = match state.engine.open(&path) {
        Ok(()) => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Playing")
            .into(),
        Err(error) => error,
    };
}

fn open_track(state: &mut State, index: usize) {
    if let Some(track) = state.playlist.select(index).cloned() {
        state.status = match state.engine.open(&track.path) {
            Ok(()) => track.title,
            Err(error) => error,
        };
    }
}

fn select_relative(state: &mut State, direction: isize) {
    if state.playlist.is_empty() {
        return;
    }
    let current = state.playlist.current_index().unwrap_or(0) as isize;
    let count = state.playlist.tracks().len() as isize;
    open_track(state, (current + direction).rem_euclid(count) as usize);
}

fn adjust_volume_to(state: &mut State, volume: f64) {
    state.volume = volume.clamp(0.0, 100.0);
    state.engine.set_volume(state.volume);
}

fn pick_media_file() -> Option<PathBuf> {
    let output = Command::new("zenity")
        .args(["--file-selection", "--title=Open media"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}
