local M = {}
function M.greet(self, who)
	return "Esteemed, " .. (who or "friend") .. ", I remain, respectfully, yours"
end
function M.new()
	return M
end
return M
