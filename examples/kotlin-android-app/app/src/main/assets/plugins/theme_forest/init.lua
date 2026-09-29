-- Second theme: warm forest greens. Proves switching needs no recompile.
local ThemeForest = {}
ThemeForest.__index = ThemeForest

function ThemeForest.new(_) return setmetatable({}, ThemeForest) end

function ThemeForest:theme()
  return {
    name = "Forest",
    colors = { background = "#101A12", surface = "#1C2B1F", primary = "#95D5B2" },
    fonts = { body = "serif", display = "serif", scale = 1.0 },
  }
end

return ThemeForest
