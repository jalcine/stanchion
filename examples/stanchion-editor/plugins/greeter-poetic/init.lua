-- Greeter-poetic: lyrical greeting
local M = {}
function M.greet(who)
  local cfg = M.config or {}
  return (cfg.prefix or "O, bright star") .. " of " .. (who or "night") .. ", " .. (cfg.suffix or "May your light endure")
end
return M
