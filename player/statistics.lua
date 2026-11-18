local stats = veyra.get_statistics()
veyra.log("position=" .. tostring(stats.position) .. ", duration=" .. tostring(stats.duration))
veyra.log("video=" .. tostring(stats.video.codec) .. " " .. tostring(stats.video.width) .. "x" .. tostring(stats.video.height))
veyra.log("audio=" .. tostring(stats.audio.codec) .. " " .. tostring(stats.audio.sample_rate) .. "Hz")

veyra.on("playback_started", function(event)
    veyra.log("statistics tool observed playback start for " .. tostring(event.file_name))
end)
