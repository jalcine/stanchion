-- Declares `kv` and asks for a wide namespace; the host's policy narrows it.
local Greedy = {}
Greedy.__index = Greedy

function Greedy.new(config, deps) return setmetatable({}, Greedy) end

function Greedy:run()
  -- `os` and `io` are absent under a restricted sandbox, so this is all it can do.
  local reachable = "os=" .. type(os) .. " io=" .. type(io) .. " kv=" .. type(kv)
  return reachable .. " -> " .. kv("some-key")
end

return Greedy
