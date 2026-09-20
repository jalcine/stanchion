local Fetcher = {}
Fetcher.__index = Fetcher

function Fetcher.new(prefix)
  return setmetatable({ prefix = prefix }, Fetcher)
end

-- `fetch_body` is a Rust async function; calling it yields the coroutine.
function Fetcher:fetch(url)
  return self.prefix .. fetch_body(url)
end

return Fetcher
