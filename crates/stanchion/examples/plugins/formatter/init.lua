-- A plugin other plugins depend on: it publishes an `exports` table.
local Formatter = {}
Formatter.__index = Formatter

function Formatter.new(config, deps)
  return setmetatable({ prefix = config.prefix }, Formatter)
end

function Formatter:name() return "formatter" end

function Formatter:handle(kind, payload)
  return self.prefix .. " " .. kind .. ": " .. payload
end

-- Only what is listed here is visible to dependents.
function Formatter:exports()
  local prefix = self.prefix
  return {
    decorate = function(text) return prefix .. " " .. text end,
  }
end

return Formatter
