use super::{api, events::EventPayload};
use mlua::{Function, Lua, RegistryKey, Result, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
};
use veyra_statistics::StatisticsSnapshot;

pub(crate) enum Command {
    LoadAll,
    Reload,
    Event(EventPayload, StatisticsSnapshot),
    Eval(String),
    Shutdown,
}

pub(crate) struct RuntimeHandle {
    tx: mpsc::Sender<Command>,
    join: Option<thread::JoinHandle<()>>,
}

impl RuntimeHandle {
    pub fn start(dir: PathBuf, stats: StatisticsSnapshot) -> Self {
        let (tx, rx) = mpsc::channel();
        let initial = stats.clone();
        let join = thread::Builder::new()
            .name("veyra-lua-runtime".into())
            .spawn(move || {
                let lua = Lua::new();
                let callbacks = Arc::new(Mutex::new(Vec::new()));
                let current = Arc::new(Mutex::new(initial));
                let _ = install_api(&lua, Arc::clone(&callbacks), Arc::clone(&current));
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::LoadAll | Command::Reload => {
                            let errors = load_tools(&lua, &dir, &callbacks).unwrap_or_default();
                            for error in errors {
                                eprintln!("veyra script: {error}");
                            }
                        }
                        Command::Event(payload, snapshot) => {
                            *current.lock().unwrap() = snapshot.clone();
                            dispatch(&lua, &callbacks, &payload);
                        }
                        Command::Eval(source) => evaluate_console(&lua, &source),
                        Command::Shutdown => break,
                    }
                }
            })
            .expect("Lua runtime thread");
        let handle = Self {
            tx,
            join: Some(join),
        };
        let _ = handle.tx.send(Command::LoadAll);
        let _ = handle.tx.send(Command::Event(
            EventPayload {
                event: "initialised".into(),
                position: stats.position,
                file_name: stats.file_name.clone(),
            },
            stats,
        ));
        handle
    }
    pub fn reload(&self) {
        let _ = self.tx.send(Command::Reload);
    }
    pub fn event(&self, payload: EventPayload, stats: StatisticsSnapshot) {
        let _ = self.tx.send(Command::Event(payload, stats));
    }
    pub fn eval(&self, source: String) {
        let _ = self.tx.send(Command::Eval(source));
    }
}
impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn load_tools(
    lua: &Lua,
    dir: &Path,
    callbacks: &Arc<Mutex<Vec<(String, RegistryKey)>>>,
) -> Result<Vec<String>> {
    callbacks.lock().unwrap().clear();
    let mut errors = Vec::new();
    load_tools_from_directory(lua, &dir.join("tools"), &mut errors)?;
    load_tools_from_directory(lua, &dir.join("player"), &mut errors)?;
    Ok(errors)
}

fn load_tools_from_directory(lua: &Lua, dir: &Path, errors: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_tools_from_directory(lua, &path, errors)?;
        } else if path.extension().is_some_and(|x| x == "lua") {
            match std::fs::read_to_string(&path) {
                Ok(source) => load_source(lua, &path.display().to_string(), &source, errors),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
    }
    Ok(())
}

fn load_source(lua: &Lua, name: &str, source: &str, errors: &mut Vec<String>) {
    if let Err(error) = lua.load(source).set_name(name).exec() {
        errors.push(format!("{name}: {error}"));
    }
}

fn dispatch(lua: &Lua, callbacks: &Arc<Mutex<Vec<(String, RegistryKey)>>>, payload: &EventPayload) {
    let Ok(value) = lua.create_table() else {
        return;
    };
    let _ = value.set("event", payload.event.clone());
    let _ = value.set("position", payload.position);
    let _ = value.set("file_name", payload.file_name.clone());
    for (name, key) in callbacks
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == &payload.event)
    {
        let Ok(function) = lua.registry_value::<Function>(key) else {
            continue;
        };
        if let Err(error) = function.call::<()>(value.clone()) {
            eprintln!("veyra script callback {name}: {error}");
        }
    }
}

fn evaluate_console(lua: &Lua, source: &str) {
    eprintln!("veyra lua> {source}");
    match lua.load(source).set_name("console").eval::<Value>() {
        Ok(Value::Nil) => {}
        Ok(value) => eprintln!("veyra lua = {value:?}"),
        Err(error) => eprintln!("veyra lua error: {error}"),
    }
}

fn install_api(
    lua: &Lua,
    callbacks: Arc<Mutex<Vec<(String, RegistryKey)>>>,
    current: Arc<Mutex<StatisticsSnapshot>>,
) -> Result<()> {
    let veyra = lua.create_table()?;
    veyra.set("_API_VERSION", api::API_VERSION)?;
    veyra.set(
        "version",
        lua.create_function(|_, ()| Ok(env!("CARGO_PKG_VERSION")))?,
    )?;
    veyra.set(
        "log",
        lua.create_function(|_, message: String| {
            eprintln!("veyra lua: {message}");
            Ok(())
        })?,
    )?;
    let benchmark_current = Arc::clone(&current);
    veyra.set(
        "benchmark",
        lua.create_function(move |_, ()| {
            let stats = benchmark_current.lock().unwrap();
            eprintln!(
                "veyra benchmark: file={} processed={} rendered={} render_fps={:?} frame_interval_ms={:?} jitter_ms={:?} conversion_current_ms={:?} conversion_avg_ms={:?} conversion_peak_ms={:?} dropped_frames={:?}",
                stats.file_name.as_deref().unwrap_or("<none>"),
                stats.video_frames_processed,
                stats.rendered_frames,
                stats.render_fps,
                stats.frame_interval_ms,
                stats.frame_jitter_ms,
                stats.frame_processing_ms,
                stats.frame_processing_avg_ms,
                stats.frame_processing_peak_ms,
                stats.dropped_frames,
            );
            Ok(())
        })?,
    )?;
    let info_current = Arc::clone(&current);
    veyra.set(
        "get_playback_info",
        lua.create_function(move |lua, ()| {
            api::playback_table(lua, &info_current.lock().unwrap())
        })?,
    )?;
    let stats_current = Arc::clone(&current);
    veyra.set(
        "get_statistics",
        lua.create_function(move |lua, ()| {
            api::statistics_table(lua, &stats_current.lock().unwrap())
        })?,
    )?;
    let callback_store = Arc::clone(&callbacks);
    veyra.set(
        "on",
        lua.create_function(move |lua, (event, function): (String, Function)| {
            const EVENTS: &[&str] = &[
                "playback_started",
                "playback_paused",
                "playback_resumed",
                "playback_stopped",
                "playback_finished",
                "playback_seeked",
                "playback_speed_changed",
                "track_changed",
            ];
            if !EVENTS.contains(&event.as_str()) {
                return Err(mlua::Error::external(format!(
                    "unknown playback event: {event}"
                )));
            }
            let key = lua.create_registry_value(function)?;
            callback_store.lock().unwrap().push((event, key));
            Ok(())
        })?,
    )?;
    lua.globals().set("veyra", veyra)
}

pub fn script_directory() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("veyra")
            .join("veyra")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/veyra")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("veyra")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_api_registers_callbacks_and_rejects_unknown_events() {
        let lua = Lua::new();
        let callbacks = Arc::new(Mutex::new(Vec::new()));
        let current = Arc::new(Mutex::new(StatisticsSnapshot::default()));
        install_api(&lua, callbacks, current).unwrap();
        lua.load(r#"veyra.on("playback_started", function(event) return event.position end)"#)
            .exec()
            .unwrap();
        assert!(
            lua.load(r#"veyra.on("not_an_event", function() end)"#)
                .exec()
                .is_err()
        );
    }
}
