--!nonstrict

local function fail(message)
  noctalia.state.set("telemetry.error", message)
end

if not noctalia.commandExists("gpuflo") then
  fail("gpuflo is not available on PATH")
else
  local started = noctalia.runStream("gpuflo --json-stream", function(line)
    local snapshot, err = noctalia.json.decode(line)
    if snapshot == nil then
      fail("Invalid GPUFlo JSON: " .. tostring(err))
    elseif snapshot.schema_version ~= 1 or type(snapshot.gpus) ~= "table" then
      fail("Unsupported GPUFlo snapshot schema")
    else
      noctalia.state.set("telemetry.error", "")
      noctalia.state.set("telemetry.snapshot", snapshot)
    end
  end)

  if not started then
    fail("Could not start gpuflo --json-stream")
  end
end
