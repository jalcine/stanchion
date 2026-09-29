-- Feature flags. The host Policy decides whether this plugin may answer;
-- an unsigned build gets the safe default (everything off) instead.
local Flags = {}
Flags.__index = Flags

function Flags.new(_) return setmetatable({}, Flags) end

function Flags:flags() return { newHeader = true } end

return Flags
