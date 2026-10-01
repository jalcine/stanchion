local M = {}
function M.greet(self, who)
	return "Hey, " .. (who or "there") .. ", Cheers!"
end
function M.new()
	return M
end
return M
