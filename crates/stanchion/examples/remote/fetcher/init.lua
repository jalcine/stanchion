-- Runs in a child process. `http` is answered by the application that launched it.
local Fetcher = {}
Fetcher.__index = Fetcher

function Fetcher.new(config, deps) return setmetatable({}, Fetcher) end

function Fetcher:fetch(url) return http(url) end

function Fetcher:crash() os.exit(9) end

return Fetcher
