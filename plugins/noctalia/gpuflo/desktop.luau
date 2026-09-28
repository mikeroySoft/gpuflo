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

local function clamp(percent)
  return math.max(0, math.min(100, percent or 0)) / 100
end

local function numberText(number, suffix)
  return number and string.format("%.0f%s", number, suffix) or "--"
end

local function meter(label, amount, color)
  return ui.column({ gap = 5 }, {
    ui.row({ justify = "space_between", align = "center" }, {
      ui.label({ text = label, color = "on_surface_variant", fontSize = 12 }),
      ui.label({ text = numberText(amount, "%"), color = "on_surface", fontWeight = "bold" }),
    }),
    ui.progress({
      progress = clamp(amount),
      fill = color,
      track = "surface_container_highest",
      height = 7,
      radius = 4,
    }),
  })
end

local function card(gpu)
  local partition = primaryPartition(gpu)
  local activity = partition and value(partition.activity_percent)
  local memory = partition and partition.memory and value(partition.memory.occupancy_percent)
  local temperature = gpu.temperature and value(gpu.temperature.hotspot_celsius)
  local power = gpu.power and value(gpu.power.socket_watts)
  local clock = partition and value(partition.gfx_clock_mhz)
  local health = gpu.health and gpu.health.message or "Health unavailable"

  return ui.column({
    gap = 11,
    padding = 14,
    minWidth = 300,
    fill = "surface/0.82",
    radius = 14,
    border = "outline_variant/0.55",
    borderWidth = 1,
  }, {
    ui.row({ gap = 9, align = "center" }, {
      ui.glyph({ name = "device-desktop-analytics", size = 20, color = "primary" }),
      ui.column({ gap = 1, flexGrow = 1 }, {
        ui.label({ text = gpu.name or "AMD GPU", fontWeight = "bold", maxLines = 1 }),
        ui.label({ text = gpu.bdf or "GPU", fontSize = 11, color = "on_surface_variant" }),
      }),
    }),
    meter("Activity", activity, "primary"),
    meter("VRAM", memory, "tertiary"),
    ui.row({ justify = "space_between", align = "center" }, {
      ui.label({ text = numberText(temperature, "°C") .. "  HOTSPOT", fontSize = 12 }),
      ui.label({ text = numberText(power, " W") .. "  POWER", fontSize = 12 }),
      ui.label({ text = numberText(clock, " MHz"), fontSize = 12 }),
    }),
    ui.separator({ color = "outline_variant/0.55" }),
    ui.label({ text = health, fontSize = 12, color = "on_surface_variant", maxLines = 2 }),
  })
end

local function render()
  if failure ~= "" then
    desktopWidget.render(ui.column({
      gap = 8,
      padding = 14,
      minWidth = 280,
      fill = "surface/0.82",
      radius = 14,
      border = "error/0.6",
      borderWidth = 1,
    }, {
      ui.label({ text = "GPUFlo", fontWeight = "bold", color = "error" }),
      ui.label({ text = failure, color = "on_surface_variant", maxLines = 3 }),
    }))
    return
  end

  local cards = {}
  if snapshot and type(snapshot.gpus) == "table" then
    for _, gpu in ipairs(snapshot.gpus) do
      table.insert(cards, card(gpu))
    end
  end

  if #cards == 0 then
    cards = {
      ui.column({ padding = 14, fill = "surface/0.82", radius = 14 }, {
        ui.label({ text = "Waiting for GPUFlo telemetry", color = "on_surface_variant" }),
      }),
    }
  end

  desktopWidget.render(ui.column({ gap = 12, minWidth = 300 }, cards))
end

noctalia.state.watch("telemetry.snapshot", function(nextSnapshot)
  snapshot = nextSnapshot
  render()
end)

noctalia.state.watch("telemetry.error", function(message)
  failure = message or ""
  render()
end)

render()
