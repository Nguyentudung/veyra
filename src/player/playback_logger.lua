-- Veyra Lua API 1.0: log the playback lifecycle.
veyra.on("playback_started", function(event) veyra.log("started: " .. tostring(event.file_name)) end)
veyra.on("playback_paused", function() veyra.log("paused") end)
veyra.on("playback_resumed", function() veyra.log("resumed") end)
veyra.on("playback_stopped", function() veyra.log("stopped") end)
