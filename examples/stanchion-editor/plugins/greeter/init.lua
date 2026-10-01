local M = {}
function M.greet(self, who)
	return "Hello, " .. (who or "world") .. " from Tauri + Stanchion!"
end
function M.new()
	return M
end
return M
