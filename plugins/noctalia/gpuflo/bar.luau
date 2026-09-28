--!nonstrict

local snapshot = noctalia.state.get("telemetry.snapshot")
local failure = noctalia.state.get("telemetry.error") or ""

local function value(observation)
  if type(observation) == "table" and type(observation.value) == "number" then
    return observation.value
  end
  return nil
end

local function primaryPartition(gpu)
  if type(gpu.partitions) ~= "table" then
    return nil
  end
  for _, partition in ipairs(gpu.partitions) do
    if partition.is_primary then
      return partition
    end
  end
  return gpu.partitions[1]
end

local function numberText(number, suffix)
  return number and string.format("%.0f%s", number, suffix) or "--"
end

local function render()
  barWidget.setGlyph("device-desktop-analytics")

  if failure ~= "" then
    barWidget.setText("GPUFlo")
    barWidget.setColor("error")
    barWidget.setGlyphColor("error")
    barWidget.setTooltip(failure)
    return
  end

  local gpu = snapshot and snapshot.gpus and snapshot.gpus[1]
  local partition = gpu and primaryPartition(gpu)
  if gpu == nil or partition == nil then
    barWidget.setText("GPU --")
    barWidget.setColor("on_surface")
    barWidget.setGlyphColor("on_surface")
    barWidget.setTooltip("Waiting for GPUFlo telemetry")
    return
  end

  local activity = value(partition.activity_percent)
  local memory = partition.memory and value(partition.memory.occupancy_percent)
  local temperature = gpu.temperature and value(gpu.temperature.hotspot_celsius)
  local power = gpu.power and value(gpu.power.socket_watts)

  barWidget.setColor("on_surface")
  barWidget.setGlyphColor("primary")
  if barWidget.isVertical() then
    barWidget.setText(numberText(activity, "%"))
  else
    barWidget.setText("GPU " .. numberText(activity, "%") .. "  " .. numberText(temperature, "°C"))
  end

  local health = gpu.health and gpu.health.message or "health unavailable"
  barWidget.setTooltip(string.format(
    "%s\nActivity %s · VRAM %s · Power %s\n%s",
    gpu.name or "AMD GPU",
    numberText(activity, "%"),
    numberText(memory, "%"),
    numberText(power, " W"),
    health
  ))
end

noctalia.state.watch("telemetry.snapshot", function(nextSnapshot)
  snapshot = nextSnapshot
  render()
end)

noctalia.state.watch("telemetry.error", function(message)
  failure = message or ""
  render()
end)

function onClick()
  noctalia.runInTerminal("gpuflo")
end

render()
