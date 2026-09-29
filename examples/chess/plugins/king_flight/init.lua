-- "King flight": in check, walk the king somewhere safe. The unglamorous move
-- that keeps the game alive — checked kings must move, capture, or block, and
-- a safe step is usually the cleanest of the three.
local KingFlight = {}
KingFlight.__index = KingFlight

function KingFlight.new(config, deps)
  return setmetatable({}, KingFlight)
end

function KingFlight:name() return "King Flight" end

function KingFlight:suggest(position)
  if not position.in_check then return nil end
  for _, move in ipairs(position.moves) do
    if move.piece == "K" and not move.to_is_attacked then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 400,
        name = "King Flight",
        rationale = move.san .. " steps the king out of the check.",
      }
    end
  end
  return nil
end

return KingFlight
