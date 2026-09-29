-- Spanish strings. Same keys as strings_en; the host picks by language.
local StringsEs = {}
StringsEs.__index = StringsEs

function StringsEs.new(_) return setmetatable({}, StringsEs) end

local TEXT = {
  greeting = "¡Hola, {name}!",
  subtitle = "Tematizado por plugins, traducido por plugins.",
  header_new = "Encabezado nuevo (flag activado)",
  header_old = "Encabezado clásico",
}

function StringsEs:text(key) return TEXT[key] end

return StringsEs
