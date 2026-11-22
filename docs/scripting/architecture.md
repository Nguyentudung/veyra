# Scripting architecture

Veyra starts one dedicated Lua runtime thread. Lua tools are discovered only from the platform configuration `tools/` directory and pre-written player commands from its sibling `player/` directory. Repository examples live in `src/player/`; both runtime directories are loaded on that thread. The UI and media worker communicate with it through a channel, so tool execution cannot run on either time-sensitive thread.

The dependency is `mlua 0.11.6` with `lua55`, `vendored`, and `send`. Its vendored `lua-src 550.0.0` package contains Lua 5.5.0. The API is Lua 5.5-compatible, but this checkout cannot claim the requested upstream Lua 5.5.1 patch level without a vendored 5.5.1 source package; it documents that limitation rather than silently presenting 5.5.0 as 5.5.1.
