-- Signed by the first party, so policy grants it `kv`.
local Shipped = {}
Shipped.__index = Shipped

function Shipped.new(config, deps) return setmetatable({}, Shipped) end

function Shipped:run()
  if kv == nil then return "no kv" end
  return "kv=" .. type(kv) .. " -> " .. kv("some-key")
end

return Shipped
