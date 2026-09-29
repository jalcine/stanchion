-- "No trades when behind": down material, keep pieces on — every swap helps the
-- side that is winning. Picks a quiet, safe move and holds the tension.
local AvoidTradeBehind = {}
AvoidTradeBehind.__index = AvoidTradeBehind

function AvoidTradeBehind.new(config, deps)
  return setmetatable({}, AvoidTradeBehind)
end

function AvoidTradeBehind:name() return "No Trades When Behind" end

function AvoidTradeBehind:suggest(position)
  local mat = position.material or {}
  local foe = "b"
  if position.side == "b" then foe = "w" end
  if (mat[foe] or 0) - (mat[position.side] or 0) < 3 then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.captured_value == 0 and not move.to_is_attacked then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 180,
        name = "No Trades When Behind",
        rationale = move.san .. " keeps pieces on — no helping the leader trade.",
      }
    end
  end
  return nil
end

return AvoidTradeBehind
