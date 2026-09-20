-- Loads fine, then fails on every call: one plugin erroring must not stop the rest.
local Grumpy = {}
Grumpy.__index = Grumpy

function Grumpy.new(config, deps) return setmetatable({}, Grumpy) end
function Grumpy:name() return "grumpy" end
function Grumpy:handle(kind, payload) error("grumpy refuses to handle " .. kind) end

return Grumpy
