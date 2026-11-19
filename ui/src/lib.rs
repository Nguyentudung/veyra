use self::lucide::{
    FAST_FORWARD, FOLDER_OPEN, PAUSE, PLAY, REWIND, SKIP_BACK, SKIP_FORWARD, VOLUME_2, VOLUME_X,
};
use self::theme::glass_capsule;
use rune::color::hex;
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::{
    AppSpec, Cx, El, NativeSurface, SurfaceFit, TextStyle, Tx, Waker, col, el, icon, row, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, PaletteSeed,
};
use std::{path::PathBuf, process::Command, time::Duration};
use veyra_playback::{Engine, VideoFrame};
use veyra_playlist::{Playlist, is_probably_media};
use veyra_scripting::{PlaybackEvent, ScriptManager};
use veyra_statistics::{StatisticsCollector, StatisticsSnapshot};
use veyra_utils::format_time;

mod component_impl;
mod lucide;
mod options_menu;
mod playback_controls;
mod statistics_panel;
mod theme;
mod titlebar;
mod top_bar;

pub struct State {
    engine: Engine,
    playlist: Playlist,
    surface: NativeSurface,
    status: String,
    volume: f64,
    frame_id: u64,
    last_paused: bool,
    last_finished: bool,
    last_muted: bool,
    last_error: Option<String>,
    last_position: f64,
    last_duration: Option<f64>,
    controls_visible: bool,
    volume_hovered: bool,
    statistics_open: bool,
    collector: StatisticsCollector,
    statistics: StatisticsSnapshot,
    scripts: ScriptManager,
    last_file_name: Option<String>,
    statistics_page: usize,
    options_open: bool,
    gpu_generation: Option<u64>,
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
            last_finished: false,
            last_muted: false,
            last_error: None,
            last_position: 0.0,
            last_duration: None,
            controls_visible: true,
            volume_hovered: false,
            statistics_open: false,
            collector: StatisticsCollector::new(),
            statistics: StatisticsSnapshot::default(),
            scripts: ScriptManager::new(StatisticsSnapshot::default()),
            last_file_name: None,
            statistics_page: 0,
            options_open: false,
            gpu_generation: None,
        }
    }
}

fn setup(state: &mut State, waker: Waker) {
    state.surface.set_waker(waker);
}

fn poll(state: &mut State, _: f64, tx: &mut Tx) -> bool {
    // Video is presented when a new frame arrives. Polling at the source's
    // ~30 FPS cadence avoids composing the same NativeSurface frame twice on
    // a 60 Hz timer; input and surface wakers still redraw immediately.
    tx.wake_after(Duration::from_millis(33));
    let mut changed = false;
    if let Some(context) = state.surface.gpu_context() {
        if state.gpu_generation != Some(context.generation) {
            state.engine.set_gpu_context(context.clone());
            state.gpu_generation = Some(context.generation);
        }
    }
    if let Some(frame) = state.engine.take_latest_frame() {
        state.collector.record_rendered_frame();
        state.frame_id = state.frame_id.wrapping_add(1);
        let result = match frame {
            VideoFrame::Bytes {
                rgba,
                width,
                height,
                ..
            } => state.surface.push_bytes(
                ByteFrame {
                    width,
                    height,
                    format: Format::Bgra,
                    color: ColorInfo::default(),
                    planes: [rgba, Vec::new()],
                },
                state.frame_id,
            ),
            VideoFrame::Gpu { frame, .. } => state.surface.push_gpu(frame, state.frame_id),
            VideoFrame::GpuVideo { frame, .. } => {
                state.surface.push_gpu_video(frame, state.frame_id)
            }
        };
        if let Err(error) = result {
            state.status = error;
        }
        changed = true;
    }
    let snapshot = state.engine.snapshot();
    state
        .collector
        .set_active(!snapshot.paused && !snapshot.finished);
    state.statistics = state.collector.sample(
        &snapshot,
        state.playlist.current_track().map(|t| t.path.as_path()),
    );
    if state.last_file_name != snapshot.file_name {
        state.collector.reset_stream();
        if snapshot.file_name.is_some() {
            let event = if state.last_file_name.is_some() {
                PlaybackEvent::TrackChanged {
                    file_name: snapshot.file_name.clone(),
                }
            } else {
                PlaybackEvent::Started
            };
            state.scripts.dispatch(event, state.statistics.clone());
        }
        state.last_file_name = snapshot.file_name.clone();
    }
    if (snapshot.position - state.last_position).abs() > 1.0 && snapshot.file_name.is_some() {
        state.scripts.dispatch(
            PlaybackEvent::Seeked {
                position: snapshot.position,
            },
            state.statistics.clone(),
        );
    }
    if state.last_paused != snapshot.paused && snapshot.file_name.is_some() {
        state.scripts.dispatch(
            if snapshot.paused {
                PlaybackEvent::Paused
            } else {
                PlaybackEvent::Resumed
            },
            state.statistics.clone(),
        );
    }
    if state.last_finished != snapshot.finished && snapshot.finished {
        state
            .scripts
            .dispatch(PlaybackEvent::Finished, state.statistics.clone());
    }
    if state.last_paused != snapshot.paused
        || state.last_muted != snapshot.muted
        || state.last_error != snapshot.error
        || (state.last_position - snapshot.position).abs() > 0.02
        || state.last_duration != snapshot.duration
    {
        state.last_paused = snapshot.paused;
        state.last_finished = snapshot.finished;
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
        rune::Key::Text(value) if value.eq_ignore_ascii_case("r") => {
            state.scripts.reload();
            state.status = "Tools reloaded".into();
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
    let root = if state.frame_id == 0 {
        // Keep the initial window focused on the empty state. Playback chrome only
        // becomes useful after a media item has produced its first video frame.
        el().w_full()
            .h_full()
            .fill(kit.c.bg_deep)
            .child(video_view(state, &kit))
    } else {
        el().w_full()
            .h_full()
            .fill(kit.c.bg_deep)
            .child(video_view(state, &kit))
            .child(top_bar::view(state, &kit))
            .child(if state.controls_visible {
                playback_controls::transport(state, &kit)
            } else {
                el()
            })
            .child(playback_controls::scrubber(state, &kit))
            .child(playback_controls::volume(state, &kit))
            .child(if state.statistics_open {
                statistics_panel::view(state, &kit)
            } else {
                el()
            })
            .child(options_menu::trigger(state, &kit))
            .child(if state.options_open {
                options_menu::view(state, &kit)
            } else {
                el()
            })
            .child(titlebar::overlay(state, &kit, cx))
    };
    let _ = cx;
    rune_kit::root(root, Vec::new())
}

fn video_view(state: &State, kit: &Kit) -> El {
    let surface = if state.frame_id > 0 {
        let snapshot = state.engine.snapshot();
        let ratio = (snapshot.video_width > 0 && snapshot.video_height > 0)
            .then_some(snapshot.video_width as f32 / snapshot.video_height as f32);
        let video = ratio.map_or_else(
            || el().w_full().h_full(),
            |ratio| el().w_full().h_full().aspect_fit(ratio),
        );
        video
            .key("video-frame")
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
                    .font(TextStyle {
                        size: 18.0,
                        ..kit.ty.ui
                    })
                    .color(kit.c.text_base),
            )
            .child(
                text(state.status.clone())
                    .font(TextStyle {
                        size: 17.0,
                        ..kit.ty.body
                    })
                    .color(kit.c.text_muted),
            )
            .child(
                Button::new("Open media")
                    .variant(ButtonVariant::Contrast)
                    .on_press(|s: &mut State| open_media(s))
                    .control_size(ControlSize::ExtraLarge)
                    .label_with(|font, ink| {
                        text("Open media")
                            .font(TextStyle { size: 18.0, ..font })
                            .color(ink)
                    })
                    .build(kit)
                    .radius(999.0),
            )
    };
    el().key("video-viewport")
        .w_full()
        .h_full()
        .center()
        .child(surface)
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
                            .fill(hex("#ffffff").unwrap()),
                    ),
            )
    } else {
        el()
    };
    col()
        // The mute capsule is 44 px tall while the playback capsule is 52 px tall.
        // Align their centers by giving the shorter capsule 8 px more bottom inset.
        .pin(None, None, Some(24.0), Some(38.0))
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
    let seek = row()
        .w_frac(0.84)
        .min_w(120.0)
        .max_w(680.0)
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
                .w_full()
                .h(6.0)
                .radius(3.0)
                .fill(hex("#ffffff26").unwrap()),
        )
        .child(
            el().grow(progress.max(0.001))
                .h(16.0)
                .child(
                    el().absolute(0.0, 5.0)
                        .w_full()
                        .h(6.0)
                        .radius(3.0)
                        .fill(kit.c.bg_accent),
                )
                .child(
                    el().pin(None, Some(2.0), Some(-6.0), None)
                        .w(12.0)
                        .h(12.0)
                        .radius(999.0)
                        .fill(hex("#ffffff").unwrap()),
                ),
        )
        .child(el().grow((1.0 - progress).max(0.001)).h(16.0));
    col()
        .pin(Some(0.0), None, Some(0.0), Some(92.0))
        .w_full()
        .center()
        .gap(8.0)
        .child(seek)
        .child(
            row()
                .w_frac(0.84)
                .min_w(120.0)
                .max_w(680.0)
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
