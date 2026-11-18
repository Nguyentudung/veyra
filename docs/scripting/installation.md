# Installing tools

Veyra creates two sibling directories on startup. Linux uses `$XDG_CONFIG_HOME/veyra/tools` and `$XDG_CONFIG_HOME/veyra/player`, or `~/.config/veyra/tools` and `~/.config/veyra/player` when that variable is absent. macOS uses `~/Library/Application Support/veyra/tools` and `~/Library/Application Support/veyra/player`; Windows uses `%APPDATA%/veyra/tools` and `%APPDATA%/veyra/player`.

Users may place additional `.lua` tools in `tools/`. Pre-written player commands belong in the sibling `player/` directory. Veyra loads `.lua` files recursively from both locations and pressing `R` reloads the complete tool tree. The repository mirrors this layout at its root with `tools/` and `player/`. The vendored source is Lua 5.5.0 through `lua-src 550.0.0`; Lua 5.5.1 is not silently substituted or claimed.
