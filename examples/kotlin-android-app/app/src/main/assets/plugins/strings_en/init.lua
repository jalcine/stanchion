-- English strings. `locale()` is the host's capability; pcall keeps this
-- fail-closed when the grant is absent, and the table tailors by region.
local StringsEn = {}
StringsEn.__index = StringsEn

function StringsEn.new(_) return setmetatable({}, StringsEn) end

local TEXT = {
  greeting = "Hello, {name}!",
  subtitle = "Themed by plugins, translated by plugins.",
  header_new = "Fresh header (flag on)",
  header_old = "Classic header",
}

function StringsEn:text(key)
  local ok, tag = pcall(function() return locale() end)
  if ok and tag == "en-GB" and key == "greeting" then
    return "Howdy, {name}!"
  end
  return TEXT[key]
end

return StringsEn
