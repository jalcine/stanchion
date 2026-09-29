-- "Activate the king": in thin endgames the king is a fighting piece — walk it
-- toward the centre. Fires only when total material is low and the king steps
-- closer to the middle.
local ActivateKing = {}
ActivateKing.__index = ActivateKing

function ActivateKing.new(config, deps)
  return setmetatable({}, ActivateKing)
end

function ActivateKing:name() return "Activate the King" end

local function center_dist(sq)
  local f = sq % 8
  local r = math.floor(sq / 8)
  return math.abs(f - 3.5) + math.abs(r - 3.5)
end

function ActivateKing:suggest(position)
  local mat = position.material or {}
  if (mat.w or 0) + (mat.b or 0) > 26 then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.piece == "K" and not move.is_castle
        and center_dist(move.to) < center_dist(move.from)
        and not move.to_is_attacked then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 200,
        name = "Activate the King",
        rationale = move.san .. " marches the king to the centre — it fights now.",
      }
    end
  end
  return nil
end

return ActivateKing
