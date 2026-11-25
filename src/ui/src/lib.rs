//! GPUI presentation layer for Veyra.
//!
//! The media worker remains isolated from the UI. GPUI consumes the portable
//! BGRA frame route, which keeps the UI toolkit boundary explicit and does
//! not require the media worker to expose compositor-owned GPU resources.

mod icons;

use gpui::{
    App, Application, Bounds, ClickEvent, Context, CursorStyle, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Render, RenderImage,
    ResizeEdge, SharedString, Timer, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions, div, img, prelude::*, px, relative, rgb, rgba, size, svg,
};
use image::{Frame, ImageBuffer, Rgba};
use smallvec::smallvec;
use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use veyra_playback::{Engine, Snapshot, VideoFrame};
use veyra_playlist::{Playlist, is_probably_media};
use veyra_scripting::{PlaybackEvent, ScriptManager};
use veyra_statistics::{StatisticsCollector, StatisticsSnapshot};
use veyra_utils::format_time;

const WINDOW_WIDTH: f32 = 1024.0;
const WINDOW_HEIGHT: f32 = 640.0;
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const UI_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const STATS_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const SCRIPT_STARTUP_DEFER: Duration = Duration::from_secs(1);

const BACKGROUND: u32 = 0x101217;
const GLASS: u32 = 0x121216cc;
const GLASS_HOVER: u32 = 0xffffff24;
const SURFACE: u32 = 0x262629;
const BORDER: u32 = 0xffffff1f;
const TEXT: u32 = 0xf2f4f8;
const MUTED: u32 = 0xaab0bd;
const FAINT: u32 = 0x7e8490;
const ACCENT: u32 = 0x8fb7ff;

pub fn run() {
    Application::new()
        .with_assets(icons::Icons)
        .run(|cx: &mut App| {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("Veyra".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Veyra".into()),
                    ..Default::default()
                }),
                window_decorations: Some(WindowDecorations::Client),
                window_min_size: Some(size(px(480.0), px(320.0))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let view = cx.new(Player::new);
                view.update(cx, |player, cx| player.start_polling(cx));
                window.focus(&view.read(cx).focus_handle(cx));
                view
            })
            .expect("open Veyra window");
            cx.activate(true);
        });
}

struct Player {
    engine: Engine,
    snapshot: Snapshot,
    playlist: Playlist,
    frame: Option<Arc<RenderImage>>,
    retired_frames: Vec<Arc<RenderImage>>,
    frame_id: u64,
    pending_track: Option<PathBuf>,
    status: String,
    volume: f64,
    controls_visible: bool,
    volume_hovered: bool,
    statistics_open: bool,
    statistics_page: usize,
    options_open: bool,
    shortcuts_open: bool,
    playlist_open: bool,
    lua_console_open: bool,
    lua_input: String,
    lua_input_focused: bool,
    collector: StatisticsCollector,
    statistics: StatisticsSnapshot,
    last_file_name: Option<String>,
    last_paused: bool,
    last_finished: bool,
    last_position: f64,
    next_ui_refresh: Instant,
    next_statistics_sample: Instant,
    scripts: Option<ScriptManager>,
    scripts_start_pending: bool,
    scripts_start_deadline: Option<Instant>,
    stream_frame_seen: bool,
    focus: FocusHandle,
}

impl Player {
    fn new(cx: &mut Context<Self>) -> Self {
        let engine = Engine::new();
        let mut player = Self {
            snapshot: engine.snapshot(),
            engine,
            playlist: Playlist::default(),
            frame: None,
            retired_frames: Vec::new(),
            frame_id: 0,
            pending_track: None,
            status: "Open a media file to begin".into(),
            volume: 100.0,
            controls_visible: true,
            volume_hovered: false,
            statistics_open: false,
            statistics_page: 0,
            options_open: false,
            shortcuts_open: false,
            playlist_open: false,
            lua_console_open: false,
            lua_input: String::new(),
            lua_input_focused: false,
            collector: StatisticsCollector::new(),
            statistics: StatisticsSnapshot::default(),
            last_file_name: None,
            last_paused: false,
            last_finished: false,
            last_position: 0.0,
            next_ui_refresh: Instant::now(),
            next_statistics_sample: Instant::now(),
            scripts: None,
            scripts_start_pending: false,
            scripts_start_deadline: None,
            stream_frame_seen: false,
            focus: cx.focus_handle(),
        };
        if let Some(path) = std::env::var_os("VEYRA_OPEN_MEDIA") {
            player.open_path(PathBuf::from(path));
        }
        player
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        cx.spawn(|this: gpui::WeakEntity<Player>, cx: &mut gpui::AsyncApp| {
            let mut app = cx.clone();
            async move {
                loop {
                    Timer::after(POLL_INTERVAL).await;
                    if this.update(&mut app, |player, cx| player.poll(cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // This task runs outside a window update, allowing GPUI to evict the
        // old textures from that window's atlas after a track change.
        for image in self.retired_frames.drain(..) {
            cx.drop_image(image, None);
        }
        let now = Instant::now();
        // The worker clears its mailbox before publishing the new path. Check
        // that path before taking a frame so a late old frame cannot replace
        // the audio placeholder while switching tracks.
        if let Some(path) = self.pending_track.as_ref() {
            let snapshot = self.engine.snapshot();
            if track_is_ready(path, &snapshot) {
                self.pending_track = None;
            }
        }
        let update = self.engine.take_frame_update();
        let mut changed = false;
        if let Some(frame) = update.frame.filter(|_| self.pending_track.is_none()) {
            changed |= self.present_frame(frame, cx);
        }

        let refresh = update.state_changed || now >= self.next_ui_refresh;
        if refresh {
            let snapshot = self.engine.snapshot();
            let stream_changed = self.last_file_name != snapshot.file_name;
            if stream_changed {
                self.collector.reset_stream();
                self.stream_frame_seen = false;
            }
            self.collector
                .set_active(!snapshot.paused && !snapshot.finished);
            let position_jumped = (snapshot.position - self.last_position).abs() > 1.0;
            let playback_changed =
                self.last_paused != snapshot.paused || self.last_finished != snapshot.finished;
            if stream_changed
                || position_jumped
                || playback_changed
                || now >= self.next_statistics_sample
            {
                self.statistics = self.collector.sample(
                    &snapshot,
                    self.playlist
                        .current_track()
                        .map(|track| track.path.as_path()),
                );
                self.write_benchmark_metrics(snapshot.position);
                self.next_statistics_sample = now + STATS_SAMPLE_INTERVAL;
            }
            if stream_changed {
                self.handle_stream_change(&snapshot, now);
            }
            if position_jumped && snapshot.file_name.is_some() && self.scripts.is_some() {
                self.dispatch_script_event(PlaybackEvent::Seeked {
                    position: snapshot.position,
                });
            }
            if self.last_paused != snapshot.paused
                && snapshot.file_name.is_some()
                && self.scripts.is_some()
            {
                self.dispatch_script_event(if snapshot.paused {
                    PlaybackEvent::Paused
                } else {
                    PlaybackEvent::Resumed
                });
            }
            if self.last_finished != snapshot.finished
                && snapshot.finished
                && self.scripts.is_some()
            {
                self.dispatch_script_event(PlaybackEvent::Finished);
            }
            if let Some(error) = snapshot.error.clone() {
                self.status = error;
            }
            self.last_paused = snapshot.paused;
            self.last_finished = snapshot.finished;
            self.last_position = snapshot.position;
            self.snapshot = snapshot;
            self.next_ui_refresh = now + UI_REFRESH_INTERVAL;
            changed = true;
        }
        self.start_deferred_scripts_if_ready(now);
        if changed {
            cx.notify();
        }
    }

    fn present_frame(&mut self, frame: VideoFrame, cx: &mut Context<Self>) -> bool {
        let timestamp = frame.timestamp();
        self.collector.record_rendered_frame();
        self.snapshot.position = timestamp;
        let VideoFrame::Bytes {
            rgba,
            width,
            height,
            ..
        } = frame;
        let Some(buffer) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, rgba) else {
            self.status = "Received an invalid video frame".into();
            return true;
        };
        // GPUI's render atlas stores image pixels in BGRA. FFmpeg's software
        // renderer already provides that exact byte order.
        let image = Arc::new(RenderImage::new(smallvec![Frame::new(buffer)]));
        if let Some(old) = self.frame.replace(image) {
            cx.drop_image(old, None);
        }
        let first_frame = self.frame_id == 0;
        self.frame_id = self.frame_id.wrapping_add(1);
        self.stream_frame_seen = true;
        if first_frame {
            self.mark_benchmark_ready();
        }
        true
    }

    fn handle_stream_change(&mut self, snapshot: &Snapshot, now: Instant) {
        if let Some(file_name) = snapshot.file_name.clone() {
            if self.scripts.is_some() {
                let event = if self.last_file_name.is_some() {
                    PlaybackEvent::TrackChanged {
                        file_name: Some(file_name),
                    }
                } else {
                    PlaybackEvent::Started
                };
                self.dispatch_script_event(event);
            } else {
                self.scripts_start_pending = true;
                self.scripts_start_deadline = Some(now + SCRIPT_STARTUP_DEFER);
            }
        } else {
            self.scripts_start_pending = false;
            self.scripts_start_deadline = None;
        }
        self.last_file_name = snapshot.file_name.clone();
    }

    fn ensure_scripts(&mut self) -> &ScriptManager {
        if self.scripts.is_none() {
            self.scripts = Some(ScriptManager::new(self.statistics.clone()));
        }
        self.scripts
            .as_ref()
            .expect("script manager is initialized")
    }

    fn dispatch_script_event(&mut self, event: PlaybackEvent) {
        let statistics = self.statistics.clone();
        self.ensure_scripts().dispatch(event, statistics);
    }

    fn start_deferred_scripts_if_ready(&mut self, now: Instant) {
        if !self.scripts_start_pending
            || (!self.stream_frame_seen
                && self
                    .scripts_start_deadline
                    .is_some_and(|deadline| now < deadline))
        {
            return;
        }
        self.scripts_start_pending = false;
        self.scripts_start_deadline = None;
        self.dispatch_script_event(PlaybackEvent::Started);
    }

    fn reload_scripts(&mut self) {
        self.scripts_start_pending = false;
        self.scripts_start_deadline = None;
        self.ensure_scripts().reload();
        self.status = "Tools reloaded".into();
    }

    fn eval_lua(&mut self) {
        let source = self.lua_input.trim().to_owned();
        if source.is_empty() {
            return;
        }
        self.ensure_scripts().eval(source);
        self.lua_input.clear();
        self.status = "Lua command sent; inspect the launch terminal".into();
    }

    fn open_media(&mut self) {
        if let Some(path) = pick_media_file() {
            self.open_path(path);
        }
    }

    fn clear_frame(&mut self) {
        if let Some(old) = self.frame.take() {
            self.retired_frames.push(old);
        }
        self.frame_id = 0;
        self.pending_track = None;
        self.stream_frame_seen = false;
    }

    fn open_path(&mut self, path: PathBuf) {
        if !is_probably_media(&path) {
            self.status = "Unsupported file type".into();
            return;
        }
        self.status = match self.engine.open(&path) {
            Ok(()) => {
                let index = self.playlist.add(path.clone());
                self.playlist.select(index);
                self.clear_frame();
                self.pending_track = Some(path.clone());
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Playing")
                    .into()
            }
            Err(error) => error,
        };
    }

    fn select_relative(&mut self, direction: isize) {
        if self.playlist.is_empty() {
            return;
        }
        let current = self.playlist.current_index().unwrap_or(0) as isize;
        let count = self.playlist.tracks().len() as isize;
        let index = (current + direction).rem_euclid(count) as usize;
        self.select_track(index);
    }

    fn select_track(&mut self, index: usize) {
        if let Some(track) = self.playlist.tracks().get(index).cloned() {
            self.status = match self.engine.open(&track.path) {
                Ok(()) => {
                    self.playlist.select(index);
                    self.clear_frame();
                    self.pending_track = Some(track.path);
                    track.title
                }
                Err(error) => error,
            };
        }
    }

    fn toggle_playback(&mut self) {
        if self.snapshot.finished {
            if let Some(index) = self.playlist.current_index() {
                self.select_track(index);
            }
        } else if !self.playlist.is_empty() {
            self.engine.toggle_pause();
        }
    }

    fn adjust_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 100.0);
        self.engine.set_volume(self.volume);
    }

    fn seek_at(&mut self, x: f32, width: f32) {
        if let Some(duration) = self.snapshot.duration.filter(|value| *value > 0.0) {
            let fraction = seek_fraction(x, width) as f64;
            self.engine.seek_to(duration * fraction);
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.control && key == "o" && !self.lua_input_focused {
            self.open_media();
            cx.notify();
            return;
        }
        if self.lua_console_open && self.lua_input_focused {
            match key {
                "backspace" => {
                    self.lua_input.pop();
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.eval_lua();
                    cx.notify();
                    return;
                }
                "escape" => {
                    self.lua_input_focused = false;
                    cx.notify();
                    return;
                }
                _ => {
                    if let Some(character) = event.keystroke.key_char.as_deref() {
                        if !character.chars().any(char::is_control) {
                            self.lua_input.push_str(character);
                            cx.notify();
                            return;
                        }
                    }
                }
            }
        }
        match key {
            "space" => self.toggle_playback(),
            "left" => self.engine.seek(-10.0),
            "right" => self.engine.seek(10.0),
            "f" => window.toggle_fullscreen(),
            "m" => self.engine.toggle_mute(),
            "h" => self.controls_visible = !self.controls_visible,
            "escape" => {
                self.options_open = false;
                self.statistics_open = false;
                self.shortcuts_open = false;
                self.playlist_open = false;
                self.lua_console_open = false;
                self.lua_input_focused = false;
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            }
            "r" => self.reload_scripts(),
            _ => return,
        }
        cx.notify();
    }

    fn mark_benchmark_ready(&self) {
        if let Some(path) = std::env::var_os("VEYRA_BENCHMARK_READY_FILE") {
            if let Err(error) = std::fs::write(path, b"ready\n") {
                eprintln!("[benchmark] could not write Veyra ready marker: {error}");
            }
        }
    }

    fn write_benchmark_metrics(&self, position: f64) {
        let Some(path) = std::env::var_os("VEYRA_BENCHMARK_METRICS_FILE") else {
            return;
        };
        let number = |value: Option<f64>| {
            value
                .filter(|value| value.is_finite())
                .map(|value| format!("{value:.6}"))
                .unwrap_or_else(|| "null".into())
        };
        let s = &self.statistics;
        let document = format!(
            "{{\"schema_version\":2,\"video_frames_processed\":{},\"rendered_frames\":{},\"playback_position_seconds\":{},\"render_fps\":{},\"frame_interval_ms\":{},\"frame_jitter_ms\":{},\"frame_processing_ms\":{},\"frame_processing_avg_ms\":{},\"frame_processing_peak_ms\":{},\"dropped_frames\":{},\"decode_threads\":{}}}\n",
            s.video_frames_processed,
            s.rendered_frames,
            number(Some(position)),
            number(s.render_fps),
            number(s.frame_interval_ms),
            number(s.frame_jitter_ms),
            number(s.frame_processing_ms),
            number(s.frame_processing_avg_ms),
            number(s.frame_processing_peak_ms),
            s.dropped_frames
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
            s.video_decode_threads
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".into()),
        );
        let path = PathBuf::from(path);
        let temporary = path.with_extension("tmp");
        if let Err(error) =
            std::fs::write(&temporary, document).and_then(|()| std::fs::rename(&temporary, &path))
        {
            eprintln!("[benchmark] could not write playback metrics: {error}");
        }
    }

    fn title(&self) -> String {
        self.playlist
            .current_track()
            .map(|track| track.title.clone())
            .unwrap_or_else(|| "Veyra".into())
    }

    fn glass(&self) -> gpui::Div {
        div()
            .bg(rgba(GLASS))
            .border_1()
            .border_color(rgba(BORDER))
            .rounded_full()
    }

    fn top_bar(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        div()
            .absolute()
            .top(px(18.0))
            .left(px(18.0))
            .right(px(18.0))
            .h(px(52.0))
            .flex()
            .items_center()
            .gap_2()
            .child(
                self.glass()
                    .id("fullscreen")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .text_color(rgb(TEXT))
                    .on_click(cx.listener(|_, _, window, _| window.toggle_fullscreen()))
                    .child(icon("fullscreen", 18.0)),
            )
            .child(
                self.glass()
                    .id("open-top")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .child(icon("folder", 19.0))
                    .on_click(cx.listener(|player, _, _, cx| {
                        player.open_media();
                        cx.notify();
                    })),
            )
            .child(
                div().flex_1().flex().justify_center().min_w(px(0.0)).child(
                    div()
                        .max_w(px(620.0))
                        .min_w(px(120.0))
                        .px_5()
                        .py_3()
                        .rounded_full()
                        .bg(rgba(GLASS))
                        .border_1()
                        .border_color(rgba(BORDER))
                        .truncate()
                        .text_color(rgb(TEXT))
                        .text_size(px(15.0))
                        .child(self.title()),
                ),
            )
            .child(
                self.glass()
                    .id("playlist-toggle")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .child(icon("playlist", 19.0))
                    .on_click(cx.listener(|player, _, _, cx| {
                        player.playlist_open = !player.playlist_open;
                        player.statistics_open = false;
                        player.shortcuts_open = false;
                        player.lua_console_open = false;
                        cx.notify();
                    })),
            )
            .child(
                self.glass()
                    .id("statistics-toggle")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .text_color(rgb(TEXT))
                    .on_click(cx.listener(|player, _, _, cx| {
                        player.statistics_open = !player.statistics_open;
                        player.shortcuts_open = false;
                        player.lua_console_open = false;
                        player.playlist_open = false;
                        cx.notify();
                    }))
                    .child(icon("info", 19.0)),
            )
    }
    fn video_view(&self, cx: &Context<Self>, compact: bool) -> gpui::Div {
        if let Some(frame) = self.frame.clone() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x08090d))
                .child(img(frame).size_full().object_fit(ObjectFit::Contain));
        }

        if !self.playlist.is_empty() {
            let label = if self.snapshot.error.is_some() {
                "UNABLE TO PLAY"
            } else if self.snapshot.video_codec.is_some() {
                "OPENING VIDEO"
            } else {
                "NOW PLAYING"
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .pt(px(if compact { 55.0 } else { 0.0 }))
                .pb(px(if compact { 92.0 } else { 110.0 }))
                .gap_3()
                .bg(rgb(BACKGROUND))
                .child(
                    div()
                        .size(px(if compact { 48.0 } else { 76.0 }))
                        .rounded_xl()
                        .border_1()
                        .border_color(rgba(0x9ab8ff38))
                        .bg(rgba(0x8fb7ff14))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icon("music", if compact { 22.0 } else { 31.0 })
                                .text_color(rgb(ACCENT)),
                        ),
                )
                .when(!compact, |this| {
                    this.child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(ACCENT))
                            .child(label),
                    )
                })
                .child(
                    div()
                        .max_w(relative(0.70))
                        .text_center()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT))
                        .truncate()
                        .child(self.title()),
                )
                .when(self.snapshot.error.is_some(), |this| {
                    this.child(
                        div()
                            .max_w(relative(0.8))
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(rgb(MUTED))
                            .line_clamp(2)
                            .child(self.status.clone()),
                    )
                });
        }

        let has_message = self.status != "Open a media file to begin";
        div()
            .size_full()
            .relative()
            .bg(rgb(BACKGROUND))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(480.0))
                    .px_6()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .size(px(if compact { 54.0 } else { 82.0 }))
                            .rounded_xl()
                            .border_1()
                            .border_color(rgba(0x9ab8ff42))
                            .bg(rgba(0x8fb7ff12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                icon("media", if compact { 24.0 } else { 34.0 })
                                    .text_color(rgb(ACCENT)),
                            ),
                    )
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .mt_5()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(ACCENT))
                                .child("YOUR MEDIA, YOUR SPACE"),
                        )
                    })
                    .child(
                        div()
                            .mt_3()
                            .text_size(px(if compact { 21.0 } else { 28.0 }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT))
                            .child("A little more room to play."),
                    )
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .mt_2()
                                .text_size(px(14.0))
                                .text_color(rgb(MUTED))
                                .child("Open a video or audio file to get started."),
                        )
                    })
                    .when(has_message, |this| {
                        this.child(
                            div()
                                .mt_2()
                                .text_size(px(12.0))
                                .text_color(rgb(MUTED))
                                .line_clamp(2)
                                .child(self.status.clone()),
                        )
                    })
                    .child(
                        div()
                            .id("open-empty")
                            .mt(px(if compact { 16.0 } else { 24.0 }))
                            .h(px(44.0))
                            .px_5()
                            .rounded_lg()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(0x101217))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.0))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(0xaacaff)))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("folder", 18.0).text_color(rgb(0x101217)))
                            .child("Open media")
                            .on_click(cx.listener(|player, _, _, cx| {
                                player.open_media();
                                cx.notify();
                            })),
                    )
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .mt_4()
                                .text_size(px(12.0))
                                .text_color(rgb(FAINT))
                                .child("Ctrl + O  ·  MP4, MKV, MP3, FLAC and more"),
                        )
                    }),
            )
    }

    fn transport(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        let playing =
            self.snapshot.file_name.is_some() && !self.snapshot.paused && !self.snapshot.finished;
        let play_icon = if playing { "pause" } else { "play" };
        div()
            .absolute()
            .bottom(px(34.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                self.glass()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(control(
                        "previous",
                        "previous",
                        cx.listener(|player, _, _, cx| {
                            player.select_relative(-1);
                            cx.notify();
                        }),
                    ))
                    .child(control(
                        "rewind",
                        "rewind",
                        cx.listener(|player, _, _, cx| {
                            player.engine.seek(-10.0);
                            cx.notify();
                        }),
                    ))
                    .child(
                        div()
                            .id("play-pause")
                            .size(px(40.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(0x101217))
                            .cursor_pointer()
                            .hover(|this| this.opacity(0.86))
                            .child(icon(play_icon, 20.0).text_color(rgb(0x101217)))
                            .on_click(cx.listener(|player, _, _, cx| {
                                player.toggle_playback();
                                cx.notify();
                            })),
                    )
                    .child(control(
                        "forward",
                        "forward",
                        cx.listener(|player, _, _, cx| {
                            player.engine.seek(10.0);
                            cx.notify();
                        }),
                    ))
                    .child(control(
                        "next",
                        "next",
                        cx.listener(|player, _, _, cx| {
                            player.select_relative(1);
                            cx.notify();
                        }),
                    )),
            )
    }

    fn timeline(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        let progress = self
            .snapshot
            .duration
            .filter(|duration| *duration > 0.0)
            .map(|duration| (self.snapshot.position / duration).clamp(0.0, 1.0) as f32)
            .unwrap_or(0.0);
        div()
            .absolute()
            .bottom(px(92.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("timeline")
                    .w(relative(0.84))
                    .min_w(px(120.0))
                    .max_w(px(680.0))
                    .h(px(16.0))
                    .relative()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|player, event: &MouseDownEvent, window, cx| {
                            player.seek_at(
                                f32::from(event.position.x),
                                f32::from(window.viewport_size().width),
                            );
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|player, event: &MouseMoveEvent, window, cx| {
                        if event.dragging() {
                            player.seek_at(
                                f32::from(event.position.x),
                                f32::from(window.viewport_size().width),
                            );
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .absolute()
                            .top(px(5.0))
                            .left(px(0.0))
                            .right(px(0.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgba(0xffffff26)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(5.0))
                            .left(px(0.0))
                            .w(relative(progress))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(1.0))
                            .left(relative(progress))
                            .ml(px(-6.0))
                            .size(px(14.0))
                            .rounded_full()
                            .bg(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .w(relative(0.84))
                    .min_w(px(120.0))
                    .max_w(px(680.0))
                    .flex()
                    .justify_between()
                    .text_size(px(13.0))
                    .text_color(rgb(MUTED))
                    .child(format_time(self.snapshot.position))
                    .child(
                        self.snapshot
                            .duration
                            .map(format_time)
                            .unwrap_or_else(|| "—:—".into()),
                    ),
            )
    }

    fn volume(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        let muted = self.snapshot.muted;
        let level = (self.volume / 100.0).clamp(0.0, 1.0) as f32;
        div()
            .id("volume")
            .absolute()
            .right(px(24.0))
            .bottom(px(38.0))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .on_hover(cx.listener(|player, inside, _, cx| {
                player.volume_hovered = *inside;
                cx.notify();
            }))
            .when(self.volume_hovered, |this| {
                this.child(
                    div()
                        .id("volume-slider")
                        .w(px(36.0))
                        .h(px(132.0))
                        .p_2()
                        .rounded_full()
                        .bg(rgba(0x090b10e6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|player, event: &MouseDownEvent, window, cx| {
                                let local = volume_fraction(
                                    f32::from(event.position.y),
                                    f32::from(window.viewport_size().height),
                                );
                                player.adjust_volume(local as f64 * 100.0);
                                cx.notify();
                            }),
                        )
                        .on_mouse_move(cx.listener(|player, event: &MouseMoveEvent, window, cx| {
                            if event.dragging() {
                                let local = volume_fraction(
                                    f32::from(event.position.y),
                                    f32::from(window.viewport_size().height),
                                );
                                player.adjust_volume(local as f64 * 100.0);
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .w(px(6.0))
                                .h(px(104.0))
                                .rounded_full()
                                .bg(rgba(0xffffff26))
                                .flex()
                                .flex_col()
                                .justify_end()
                                .child(
                                    div()
                                        .w_full()
                                        .h(relative(level))
                                        .rounded_full()
                                        .bg(rgb(ACCENT)),
                                ),
                        ),
                )
            })
            .child(
                self.glass()
                    .id("mute")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .on_click(cx.listener(|player, _, _, cx| {
                        player.engine.toggle_mute();
                        cx.notify();
                    }))
                    .text_color(rgb(TEXT))
                    .child(icon(if muted { "muted" } else { "volume" }, 19.0)),
            )
    }

    fn options(&self, cx: &Context<Self>, compact: bool) -> impl gpui::IntoElement {
        div()
            .absolute()
            .left(px(18.0))
            .bottom(px(34.0))
            .flex()
            .flex_col()
            .gap_2()
            .when(self.options_open, |this| {
                this.child(
                    div()
                        .id("options-panel")
                        .w(px(330.0))
                        .max_h(px(if compact { 155.0 } else { 300.0 }))
                        .overflow_y_scroll()
                        .mb_2()
                        .p_2()
                        .rounded_lg()
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(menu_item(
                            "open-menu",
                            "Open media",
                            "Ctrl+O",
                            cx.listener(|player, _, _, cx| {
                                player.options_open = false;
                                player.open_media();
                                cx.notify();
                            }),
                        ))
                        .child(menu_item(
                            "reload-menu",
                            "Reload tools",
                            "R",
                            cx.listener(|player, _, _, cx| {
                                player.reload_scripts();
                                cx.notify();
                            }),
                        ))
                        .child(menu_item(
                            "lua-menu",
                            "Lua console",
                            "",
                            cx.listener(|player, _, _, cx| {
                                player.lua_console_open = !player.lua_console_open;
                                player.options_open = false;
                                cx.notify();
                            }),
                        ))
                        .child(div().my_1().h(px(1.0)).bg(rgba(BORDER)))
                        .child(menu_item(
                            "hide-menu",
                            if self.controls_visible {
                                "Hide playback buttons"
                            } else {
                                "Show playback buttons"
                            },
                            "H",
                            cx.listener(|player, _, _, cx| {
                                player.controls_visible = !player.controls_visible;
                                cx.notify();
                            }),
                        ))
                        .child(menu_item(
                            "fullscreen-menu",
                            "Fullscreen",
                            "F",
                            cx.listener(|player, _, window, cx| {
                                player.options_open = false;
                                window.toggle_fullscreen();
                                cx.notify();
                            }),
                        ))
                        .child(div().my_1().h(px(1.0)).bg(rgba(BORDER)))
                        .child(menu_item(
                            "shortcuts-menu",
                            "Keyboard shortcuts",
                            "",
                            cx.listener(|player, _, _, cx| {
                                player.options_open = false;
                                player.shortcuts_open = true;
                                cx.notify();
                            }),
                        )),
                )
            })
            .child(
                self.glass()
                    .id("options-toggle")
                    .size(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                    .text_color(rgb(TEXT))
                    .on_click(cx.listener(|player, _, _, cx| {
                        player.options_open = !player.options_open;
                        cx.notify();
                    }))
                    .child(icon("menu", 19.0)),
            )
    }

    fn statistics_panel(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        let tabs = [
            "Overview",
            "Video",
            "Audio",
            "Performance",
            "Buffering",
            "Streams",
        ];
        div()
            .absolute()
            .top(px(82.0))
            .right(px(18.0))
            .bottom(px(18.0))
            .w(px(440.0))
            .p_4()
            .rounded_xl()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgba(BORDER))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .text_color(rgb(TEXT))
                    .text_size(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Statistics")
                    .child(
                        div()
                            .id("close-stats")
                            .size(px(28.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgba(GLASS_HOVER)))
                            .text_color(rgb(TEXT))
                            .child(icon("close", 16.0))
                            .on_click(cx.listener(|player, _, _, cx| {
                                player.statistics_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap_3()
                    .child(div().w(px(112.0)).flex().flex_col().gap_1().children(
                        tabs.into_iter().enumerate().map(|(page, label)| {
                            div()
                                .id(SharedString::from(format!("stats-tab-{page}")))
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(13.0))
                                .when(page == self.statistics_page, |this| {
                                    this.bg(rgb(ACCENT)).text_color(rgb(0x101217))
                                })
                                .when(page != self.statistics_page, |this| {
                                    this.text_color(rgb(MUTED))
                                        .hover(|inner| inner.bg(rgba(GLASS_HOVER)))
                                })
                                .child(label)
                                .on_click(cx.listener(move |player, _, _, cx| {
                                    player.statistics_page = page;
                                    cx.notify();
                                }))
                        }),
                    ))
                    .child(div().w(px(1.0)).bg(rgba(0xffffff14)))
                    .child(
                        div()
                            .id("statistics-scroll")
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(self.statistics_page_view()),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(FAINT))
                    .child("Unavailable metrics are shown as —"),
            )
    }

    fn playlist_panel(&self, cx: &Context<Self>, compact: bool) -> impl gpui::IntoElement {
        div()
            .absolute()
            .top(px(80.0))
            .right(px(18.0))
            .bottom(px(if compact { 18.0 } else { 145.0 }))
            .w(px(360.0))
            .max_w(relative(0.92))
            .p_4()
            .rounded_xl()
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgba(BORDER))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child("Playlist"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(MUTED))
                                    .child(format!("{} items", self.playlist.tracks().len())),
                            ),
                    )
                    .child(
                        div()
                            .id("close-playlist")
                            .size(px(30.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgba(GLASS_HOVER)))
                            .child(icon("close", 16.0))
                            .on_click(cx.listener(|player, _, _, cx| {
                                player.playlist_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("playlist-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        self.playlist
                            .tracks()
                            .iter()
                            .enumerate()
                            .map(|(index, track)| {
                                let selected = self.playlist.current_index() == Some(index);
                                div()
                                    .id(SharedString::from(format!("playlist-item-{index}")))
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .when(selected, |this| this.bg(rgba(0x8fb7ff20)))
                                    .when(!selected, |this| {
                                        this.hover(|inner| inner.bg(rgba(GLASS_HOVER)))
                                    })
                                    .child(
                                        div()
                                            .w(px(24.0))
                                            .text_size(px(12.0))
                                            .text_color(rgb(if selected { ACCENT } else { FAINT }))
                                            .child(format!("{:02}", index + 1)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(13.0))
                                            .text_color(rgb(if selected { TEXT } else { MUTED }))
                                            .child(track.title.clone()),
                                    )
                                    .on_click(cx.listener(move |player, _, _, cx| {
                                        player.select_track(index);
                                        player.playlist_open = false;
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
    }

    fn statistics_page_view(&self) -> impl gpui::IntoElement {
        let s = &self.statistics;
        let optional = |value: Option<String>| value.unwrap_or_else(|| "—".into());
        let rows: Vec<(&str, String)> = match self.statistics_page {
            0 => vec![
                ("File", optional(s.file_name.clone())),
                ("Path", optional(s.file_path.clone())),
                ("Container", optional(s.container.clone())),
                ("Position", format_time(s.position)),
                (
                    "Remaining",
                    s.duration
                        .map(|d| format_time((d - s.position).max(0.0)))
                        .unwrap_or_else(|| "—".into()),
                ),
                ("State", s.playback_state.clone()),
                (
                    "Resolution",
                    match (s.video_width, s.video_height) {
                        (Some(w), Some(h)) => format!("{w} × {h}"),
                        _ => "—".into(),
                    },
                ),
                ("Video codec", optional(s.video_codec.clone())),
                ("Audio codec", optional(s.audio_codec.clone())),
            ],
            1 => vec![
                ("Codec", optional(s.video_codec.clone())),
                ("Profile", optional(s.video_profile.clone())),
                ("Pixel format", optional(s.video_pixel_format.clone())),
                ("FPS", number(s.video_fps, "")),
                ("Bitrate", number(s.video_bitrate.map(|v| v as f64), " bps")),
                ("Decoder", optional(s.video_decoder.clone())),
                ("Decode threads", optional_number(s.video_decode_threads)),
                ("Thread mode", optional(s.video_decode_thread_type.clone())),
                ("Frames processed", s.video_frames_processed.to_string()),
                ("Frames rendered", s.rendered_frames.to_string()),
                ("Render FPS", number(s.render_fps, "")),
                ("Frame interval", number(s.frame_interval_ms, " ms")),
                ("Frame jitter", number(s.frame_jitter_ms, " ms")),
                ("Processing", number(s.frame_processing_ms, " ms")),
            ],
            2 => vec![
                ("Codec", optional(s.audio_codec.clone())),
                ("Bitrate", number(s.audio_bitrate.map(|v| v as f64), " bps")),
                (
                    "Sample rate",
                    number(s.audio_sample_rate.map(|v| v as f64), " Hz"),
                ),
                ("Channels", optional_number(s.audio_channels)),
                ("Output", optional(s.audio_output.clone())),
                ("Processing", number(s.audio_processing_ms, " ms")),
            ],
            3 => vec![
                ("Current frame", number(s.frame_processing_ms, " ms")),
                ("Average frame", number(s.frame_processing_avg_ms, " ms")),
                ("Peak frame", number(s.frame_processing_peak_ms, " ms")),
                ("Frames processed", s.video_frames_processed.to_string()),
                ("Frames rendered", s.rendered_frames.to_string()),
                ("Render FPS", number(s.render_fps, "")),
                ("Frame interval", number(s.frame_interval_ms, " ms")),
                ("Frame jitter", number(s.frame_jitter_ms, " ms")),
                ("Dropped frames", optional_number(s.dropped_frames)),
                ("Sampling", "250 ms".into()),
            ],
            4 => vec![
                ("Buffering state", "—".into()),
                ("Packet queues", "—".into()),
                ("Buffered duration", "—".into()),
                (
                    "Note",
                    "Veyra currently decodes directly without a buffering queue.".into(),
                ),
            ],
            _ => vec![
                ("Video stream", optional(s.video_codec.clone())),
                ("Audio stream", optional(s.audio_codec.clone())),
                (
                    "Audio sample rate",
                    number(s.audio_sample_rate.map(|v| v as f64), " Hz"),
                ),
                (
                    "Note",
                    "Only selected best audio/video streams are exposed.".into(),
                ),
            ],
        };
        div()
            .text_size(px(12.0))
            .children(rows.into_iter().map(|(name, value)| stat_row(name, value)))
    }

    fn lua_console(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        div().absolute().top(px(82.0)).left(px(0.0)).right(px(0.0)).flex().justify_center()
            .child(div().w(px(520.0)).max_w(relative(0.92)).p_4().rounded_xl().bg(rgb(SURFACE)).border_1().border_color(rgba(BORDER)).flex().flex_col().gap_3()
                .child(div().flex().justify_between().items_center().text_color(rgb(TEXT)).text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD)
                    .child("Lua console").child(div().id("close-lua").size(px(28.0)).rounded_full().flex().items_center().justify_center().cursor_pointer().hover(|this| this.bg(rgba(GLASS_HOVER))).child("×")
                        .on_click(cx.listener(|player, _, _, cx| { player.lua_console_open = false; player.lua_input_focused = false; cx.notify(); }))))
                .child(div().text_size(px(12.0)).text_color(rgb(FAINT)).child("Enter a Lua expression or statement. Output and errors are printed to the terminal that launched Veyra."))
                .child(div().flex().gap_2().child(div().id("lua-input").flex_1().min_w(px(0.0)).px_3().py_2().rounded_md().bg(rgb(0x101217)).border_1().border_color(if self.lua_input_focused { rgb(ACCENT) } else { rgba(BORDER) }).cursor_text()
                    .text_color(if self.lua_input.is_empty() { rgb(FAINT) } else { rgb(TEXT) }).text_size(px(14.0)).truncate()
                    .child(if self.lua_input.is_empty() { "veyra.benchmark()".into() } else { self.lua_input.clone() })
                    .on_click(cx.listener(|player, _, window, cx| { player.lua_input_focused = true; player.focus.focus(window); cx.notify(); })))
                    .child(div().id("run-lua").px_4().py_2().rounded_md().bg(rgb(ACCENT)).text_color(rgb(0x101217)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().child("Run")
                        .on_click(cx.listener(|player, _, _, cx| { player.eval_lua(); cx.notify(); }))))
                .child(div().text_size(px(12.0)).text_color(rgb(FAINT)).child("Examples:  veyra.benchmark()    ·    veyra.log(veyra.get_statistics().render_fps)")))
    }

    fn shortcuts_panel(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
        let shortcuts = [
            ("Open media", "Ctrl + O"),
            ("Play / pause", "Space"),
            ("Seek backward / forward", "← / →"),
            ("Mute / unmute", "M"),
            ("Show / hide playback buttons", "H"),
            ("Toggle fullscreen", "F"),
            ("Close panel / exit fullscreen", "Esc"),
        ];
        div()
            .absolute()
            .top(px(72.0))
            .bottom(px(18.0))
            .left(px(16.0))
            .right(px(16.0))
            .flex()
            .justify_center()
            .items_start()
            .child(
                div()
                    .w(px(390.0))
                    .max_w(relative(1.0))
                    .max_h(relative(1.0))
                    .p_4()
                    .rounded_xl()
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child("Keyboard shortcuts"),
                            )
                            .child(
                                div()
                                    .id("close-shortcuts")
                                    .size(px(30.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|this| this.bg(rgba(GLASS_HOVER)))
                                    .child(icon("close", 16.0))
                                    .on_click(cx.listener(|player, _, _, cx| {
                                        player.shortcuts_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("shortcuts-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .children(shortcuts.into_iter().map(|(action, key)| {
                                div()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(rgb(MUTED))
                                            .child(action),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgba(0xffffff12))
                                            .text_size(px(12.0))
                                            .text_color(rgb(TEXT))
                                            .child(key),
                                    )
                            })),
                    ),
            )
    }
}

impl Focusable for Player {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let client_chrome = !window.is_fullscreen();
        let can_resize = client_chrome && !window.is_maximized();
        div()
            .id("veyra-root")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(BACKGROUND))
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(Self::handle_key))
            .when(client_chrome, |this| this.child(window_titlebar()))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(self.video_view(cx, f32::from(window.viewport_size().height) < 440.0))
                    .when(!self.playlist.is_empty(), |this| {
                        this.child(self.top_bar(cx))
                            .child(self.timeline(cx))
                            .child(self.volume(cx))
                            .child(
                                self.options(cx, f32::from(window.viewport_size().height) < 440.0),
                            )
                            .when(self.controls_visible, |this| this.child(self.transport(cx)))
                            .when(self.statistics_open, |this| {
                                this.child(self.statistics_panel(cx))
                            })
                            .when(self.playlist_open, |this| {
                                this.child(self.playlist_panel(
                                    cx,
                                    f32::from(window.viewport_size().height) < 440.0,
                                ))
                            })
                            .when(self.lua_console_open, |this| {
                                this.child(self.lua_console(cx))
                            })
                            .when(self.shortcuts_open, |this| {
                                this.child(self.shortcuts_panel(cx))
                            })
                    }),
            )
            .when(can_resize, |this| this.children(resize_handles()))
    }
}

fn window_titlebar() -> impl gpui::IntoElement {
    div()
        .h(px(42.0))
        .w_full()
        .flex()
        .items_center()
        .bg(rgb(0x181b22))
        .border_b_1()
        .border_color(rgba(BORDER))
        .child(
            div()
                .id("window-drag")
                .flex_1()
                .h_full()
                .px_4()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT))
                .child(icon("media", 16.0).text_color(rgb(ACCENT)))
                .child("Veyra")
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                }),
        )
        .child(window_button("window-minimize", "minimize", |window| {
            window.minimize_window()
        }))
        .child(window_button("window-maximize", "maximize", |window| {
            window.zoom_window()
        }))
        .child(window_button("window-close", "close", |window| {
            window.remove_window()
        }))
}

fn window_button(
    id: &'static str,
    glyph: &'static str,
    action: fn(&mut Window),
) -> impl gpui::IntoElement {
    div()
        .id(id)
        .w(px(46.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(move |this| {
            if id == "window-close" {
                this.bg(rgb(0xc83e50))
            } else {
                this.bg(rgba(GLASS_HOVER))
            }
        })
        .child(icon(glyph, 15.0))
        .on_click(move |_, window, _| action(window))
}

fn resize_handles() -> Vec<gpui::AnyElement> {
    use gpui::IntoElement;
    let h = px(6.0);
    let corner = px(12.0);
    let make = |id, edge, cursor| {
        div()
            .id(id)
            .absolute()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                window.start_window_resize(edge)
            })
    };
    vec![
        make("resize-top", ResizeEdge::Top, CursorStyle::ResizeUpDown)
            .top(px(0.0))
            .left(corner)
            .right(corner)
            .h(h)
            .into_any_element(),
        make(
            "resize-bottom",
            ResizeEdge::Bottom,
            CursorStyle::ResizeUpDown,
        )
        .bottom(px(0.0))
        .left(corner)
        .right(corner)
        .h(h)
        .into_any_element(),
        make(
            "resize-left",
            ResizeEdge::Left,
            CursorStyle::ResizeLeftRight,
        )
        .left(px(0.0))
        .top(corner)
        .bottom(corner)
        .w(h)
        .into_any_element(),
        make(
            "resize-right",
            ResizeEdge::Right,
            CursorStyle::ResizeLeftRight,
        )
        .right(px(0.0))
        .top(corner)
        .bottom(corner)
        .w(h)
        .into_any_element(),
        make(
            "resize-tl",
            ResizeEdge::TopLeft,
            CursorStyle::ResizeUpLeftDownRight,
        )
        .top(px(0.0))
        .left(px(0.0))
        .size(corner)
        .into_any_element(),
        make(
            "resize-tr",
            ResizeEdge::TopRight,
            CursorStyle::ResizeUpRightDownLeft,
        )
        .top(px(0.0))
        .right(px(0.0))
        .size(corner)
        .into_any_element(),
        make(
            "resize-bl",
            ResizeEdge::BottomLeft,
            CursorStyle::ResizeUpRightDownLeft,
        )
        .bottom(px(0.0))
        .left(px(0.0))
        .size(corner)
        .into_any_element(),
        make(
            "resize-br",
            ResizeEdge::BottomRight,
            CursorStyle::ResizeUpLeftDownRight,
        )
        .bottom(px(0.0))
        .right(px(0.0))
        .size(corner)
        .into_any_element(),
    ]
}

fn icon(name: &'static str, size: f32) -> gpui::Svg {
    svg().path(name).size(px(size)).text_color(rgb(TEXT))
}

fn volume_fraction(y: f32, viewport_height: f32) -> f32 {
    // The slider sits above the bottom-anchored mute button; the track is
    // centered within the 132 px slider, with 14 px of padding at each end.
    ((viewport_height - y - 104.0) / 104.0).clamp(0.0, 1.0)
}

fn seek_fraction(x: f32, width: f32) -> f32 {
    let track_width = (width * 0.84).clamp(120.0, 680.0);
    ((x - (width - track_width) / 2.0) / track_width).clamp(0.0, 1.0)
}

fn track_is_ready(path: &std::path::Path, snapshot: &Snapshot) -> bool {
    snapshot.error.is_none()
        && snapshot.file_path.as_deref() == Some(path.to_string_lossy().as_ref())
}

#[cfg(test)]
mod ui_tests {
    use super::{seek_fraction, track_is_ready, volume_fraction};
    use std::path::Path;
    use veyra_playback::Snapshot;

    #[test]
    fn seeking_matches_centered_track_even_when_width_is_capped() {
        assert_eq!(seek_fraction(260.0, 1200.0), 0.0);
        assert_eq!(seek_fraction(600.0, 1200.0), 0.5);
        assert_eq!(seek_fraction(940.0, 1200.0), 1.0);
        assert_eq!(seek_fraction(500.0, 1000.0), 0.5);
    }

    #[test]
    fn volume_tracks_pointer_and_clamps_outside_slider() {
        assert_eq!(volume_fraction(432.0, 640.0), 1.0);
        assert_eq!(volume_fraction(484.0, 640.0), 0.5);
        assert_eq!(volume_fraction(536.0, 640.0), 0.0);
        assert_eq!(volume_fraction(600.0, 640.0), 0.0);
    }

    #[test]
    fn pending_track_rejects_stale_frames_and_failed_open() {
        let mut snapshot = Snapshot {
            file_path: Some("/media/old.mp4".into()),
            ..Default::default()
        };
        assert!(!track_is_ready(Path::new("/media/new.mp3"), &snapshot));
        snapshot.file_path = Some("/media/new.mp3".into());
        snapshot.error = Some("failed".into());
        assert!(!track_is_ready(Path::new("/media/new.mp3"), &snapshot));
        snapshot.error = None;
        assert!(track_is_ready(Path::new("/media/new.mp3"), &snapshot));
    }
}

fn control(
    id: &'static str,
    icon: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl gpui::IntoElement {
    div()
        .id(id)
        .size(px(40.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(18.0))
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|this| this.bg(rgba(GLASS_HOVER)))
        .child(crate::icon(icon, 19.0))
        .on_click(on_click)
}

fn menu_item(
    id: &'static str,
    label: &'static str,
    shortcut: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl gpui::IntoElement {
    div()
        .id(id)
        .h(px(36.0))
        .px_3()
        .rounded_md()
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(|this| this.bg(rgba(GLASS_HOVER)))
        .text_color(rgb(TEXT))
        .text_size(px(14.0))
        .child(label)
        .child(
            div()
                .flex_1()
                .text_right()
                .text_color(rgb(FAINT))
                .text_size(px(12.0))
                .child(shortcut),
        )
        .on_click(on_click)
}

fn stat_row(name: &str, value: String) -> gpui::Div {
    div()
        .flex()
        .gap_2()
        .py(px(2.0))
        .child(
            div()
                .w(px(110.0))
                .flex_none()
                .text_color(rgb(MUTED))
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_right()
                .text_color(rgb(TEXT))
                .line_clamp(2)
                .child(value),
        )
}

fn number(value: Option<f64>, suffix: &str) -> String {
    value
        .map(|value| format!("{value:.2}{suffix}"))
        .unwrap_or_else(|| "—".into())
}

fn optional_number<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into())
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
