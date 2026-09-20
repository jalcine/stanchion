-- Never returns. In-process, without an instruction limit, this hangs the host.
local Looper = {}
Looper.__index = Looper

function Looper.new(config, deps) return setmetatable({}, Looper) end
function Looper:run() while true do end end

return Looper
