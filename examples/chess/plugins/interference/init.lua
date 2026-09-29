-- "Interference": step onto a square two enemy lines cross — the defender's ray
-- is blocked the moment you land, and the landing square itself is safe after.
-- Reads the foe_attacks map (pre-move line crossings) + to_is_attacked.
local Interference = {}
Interference.__index = Interference

function Interference.new(config, deps)
  return setmetatable({}, Interference)
end

function Interference:name() return "Interference" end

function Interference:suggest(position)
  local fa = position.foe_attacks or {}
  for _, move in ipairs(position.moves) do
    local crossing = fa[move.to] or {}
    if #crossing >= 2 and not move.to_is_attacked then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 500,
        name = "Interference",
        rationale = move.san .. " blocks the defender's line where two rays cross.",
      }
    end
  end
  return nil
end

return Interference
