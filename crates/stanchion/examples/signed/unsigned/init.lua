-- No signature artifact, so this loads as `Signer::Unsigned` and degrades
-- to no `kv` instead of failing: the capability is declared optional.
local Unsigned = {}
Unsigned.__index = Unsigned

function Unsigned.new(config, deps) return setmetatable({}, Unsigned) end

function Unsigned:run()
  if kv == nil then return "no kv" end
  return "kv=" .. type(kv) .. " -> " .. kv("some-key")
end

return Unsigned
