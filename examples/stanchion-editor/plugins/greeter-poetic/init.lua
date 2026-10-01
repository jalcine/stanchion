local M = {}
function M.greet(self, who)
	return "O, bright star of " .. (who or "night") .. ", May your light endure"
end
function M.new()
	return M
end
return M
