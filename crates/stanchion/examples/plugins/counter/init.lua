-- Wired to `formatter`: it receives only that plugin's published exports.
local Counter = {}
Counter.__index = Counter

function Counter.new(config, deps)
  return setmetatable({ seen = 0, formatter = deps.formatter }, Counter)
end

function Counter:name() return "counter" end

function Counter:handle(kind, payload)
  self.seen = self.seen + 1
  return self.formatter.decorate("event #" .. self.seen .. " (" .. kind .. ")")
end

return Counter
