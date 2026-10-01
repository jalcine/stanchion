-- Greeter-formal: formal address style
local M = {}
function M.greet(who)
  local cfg = M.config or {}
  local prefix = cfg.prefix or "Esteemed"
  local suffix = cfg.suffix or "Yours, sincerely"
  return prefix .. " " .. (who or "friend") .. ", " .. suffix
end
return M
