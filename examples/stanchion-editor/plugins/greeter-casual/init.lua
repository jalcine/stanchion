-- Greeter-casual: casual, friendly
local M = {}
function M.greet(who)
  local cfg = M.config or {}
  return (cfg.prefix or "Hey") .. " " .. (who or "there") .. "! " .. (cfg.suffix or "Cheers!")
end
return M
