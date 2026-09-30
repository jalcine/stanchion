-- Reports the message from its manifest config. Edit the message, save, and the
-- harness reloads this file and shows the new one without restarting.
local Counter = {}
Counter.__index = Counter

function Counter.new(config, deps)
  return setmetatable({ message = config.message or "unset" }, Counter)
end

function Counter:describe() return self.message end

return Counter
