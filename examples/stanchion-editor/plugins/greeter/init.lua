-- Greeter plugin: simple Lua extension for the Tauri editor
local M = {}

function M.greet(name)
  return "Hello, " .. (name or "world") .. "!"
end

function M.capabilities()
  return { "greeting" }
end

return M
