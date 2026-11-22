# Security

Lua is embedded with vendored Lua 5.5 support. Tools are loaded only from Veyra's configuration `tools/` directory and player commands from its sibling `player/` directory; repository examples are under `src/player/`. The public API intentionally has no unrestricted file, process, network, or shell access. Tool failures are isolated to the runtime thread and reported to stderr.
