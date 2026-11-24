use mlua::{Lua, Result, Table};
use veyra_statistics::StatisticsSnapshot;

pub const API_VERSION: &str = "1.0";

pub fn playback_table(lua: &Lua, stats: &StatisticsSnapshot) -> Result<Table> {
    let t = lua.create_table()?;
    t.set("file_name", stats.file_name.clone())?;
    t.set("file_path", stats.file_path.clone())?;
    t.set("position", stats.position)?;
    t.set("duration", stats.duration)?;
    t.set("state", stats.playback_state.clone())?;
    t.set("speed", stats.playback_speed)?;
    Ok(t)
}

pub fn statistics_table(lua: &Lua, stats: &StatisticsSnapshot) -> Result<Table> {
    let t = playback_table(lua, stats)?;
    let video = lua.create_table()?;
    video.set("codec", stats.video_codec.clone())?;
    video.set("width", stats.video_width)?;
    video.set("height", stats.video_height)?;
    video.set("fps", stats.video_fps)?;
    video.set("bitrate", stats.video_bitrate)?;
    video.set("profile", stats.video_profile.clone())?;
    video.set("pixel_format", stats.video_pixel_format.clone())?;
    video.set("decoder", stats.video_decoder.clone())?;
    video.set("decode_threads", stats.video_decode_threads)?;
    video.set("decode_thread_type", stats.video_decode_thread_type.clone())?;
    let audio = lua.create_table()?;
    audio.set("codec", stats.audio_codec.clone())?;
    audio.set("sample_rate", stats.audio_sample_rate)?;
    audio.set("channels", stats.audio_channels)?;
    audio.set("bitrate", stats.audio_bitrate)?;
    audio.set("output", stats.audio_output.clone())?;
    t.set("container", stats.container.clone())?;
    t.set("file_size", stats.file_size)?;
    t.set("frames_processed", stats.video_frames_processed)?;
    t.set("frames_rendered", stats.rendered_frames)?;
    t.set("render_fps", stats.render_fps)?;
    t.set("frame_interval_ms", stats.frame_interval_ms)?;
    t.set("frame_jitter_ms", stats.frame_jitter_ms)?;
    t.set("frame_processing_ms", stats.frame_processing_ms)?;
    t.set("frame_processing_avg_ms", stats.frame_processing_avg_ms)?;
    t.set("frame_processing_peak_ms", stats.frame_processing_peak_ms)?;
    t.set("audio_processing_ms", stats.audio_processing_ms)?;
    t.set("dropped_frames", stats.dropped_frames)?;
    t.set("averages", lua.create_table_from(stats.averages.clone())?)?;
    t.set("video", video)?;
    t.set("audio", audio)?;
    t.set("history", lua.create_sequence_from(stats.history.clone())?)?;
    t.set(
        "frame_history",
        lua.create_sequence_from(stats.frame_history.clone())?,
    )?;
    Ok(t)
}
