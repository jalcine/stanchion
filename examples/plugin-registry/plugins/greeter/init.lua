-- Greets whoever loads it. Requests no capabilities, reads no state.
local Greeter = {}
Greeter.__index = Greeter

function Greeter.new(config, deps) return setmetatable({}, Greeter) end

function Greeter:greet() return "hello from the registry" end

return Greeter
