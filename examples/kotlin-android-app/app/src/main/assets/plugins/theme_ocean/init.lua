-- One theme: deep-sea blues. The host discovers every `theme_*` plugin via
-- dispatch("theme") and lets the user pick; adding a theme is a new folder.
local ThemeOcean = {}
ThemeOcean.__index = ThemeOcean

function ThemeOcean.new(_) return setmetatable({}, ThemeOcean) end

function ThemeOcean:theme()
  return {
    name = "Ocean",
    colors = { background = "#0B1526", surface = "#14213D", primary = "#4CC9F0" },
    fonts = { body = "sans-serif", display = "sans-serif-medium", scale = 1.0 },
  }
end

return ThemeOcean
